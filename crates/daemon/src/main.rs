mod config;
mod setup;
mod bootstrap;
mod shutdown;

#[tokio::main]
async fn main() {
    let mut cfg = match config::PantherConfig::load().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    if !is_configured(&cfg) {
        if !atty::is(atty::Stream::Stdin) {
            eprintln!("Panther is not configured. Run 'panther-install' to set it up.");
            std::process::exit(1);
        }
        println!("Panther is not configured yet. Starting setup...");
        cfg = setup::run(cfg);
        match cfg.save().await {
            Ok(_) => println!("✓ Configuration saved."),
            Err(e) => {
                eprintln!("Failed to save config: {}", e);
                std::process::exit(1);
            }
        }
    }

    let system = match bootstrap::init(cfg).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialize Panther: {}", e);
            std::process::exit(1);
        }
    };

    println!("Panther is online.");

    let bootstrap::PantherSystem {
        agent,
        bot,
        discord_bot,
        outbound_dispatcher,
        context_engine,
        event_bus,
        cron,
        heartbeat,
        background_tasks: _background_tasks,
        ..
    } = system;

    tokio::spawn(shutdown::handle_shutdown(event_bus));
    tokio::spawn(context_engine.start());

    cron.start().await;
    heartbeat.clone().start().await;

    let agent_clone = std::sync::Arc::clone(&agent);
    tokio::spawn(async move {
        agent_clone.run().await;
    });

    tokio::spawn(async move {
        outbound_dispatcher.run().await;
    });

    if let Some(discord) = discord_bot {
        tokio::spawn(async move {
            if let Err(e) = discord.run().await {
                eprintln!("Discord bot error: {}", e);
            }
        });
    }

    if !bot.token.is_empty() {
        bot.run().await.ok();
    } else {
        tokio::signal::ctrl_c().await.ok();
    }

    cron.stop().await;
    heartbeat.stop().await;

    println!("Panther stopped.");
}

fn is_configured(cfg: &config::PantherConfig) -> bool {
    !cfg.telegram_token.is_empty()
        || cfg.discord_token.is_some()
        || cfg.slack.enabled
        || cfg.email.enabled
        || cfg.matrix.enabled
        || cfg.cli.enabled
}
