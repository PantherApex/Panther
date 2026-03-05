use shared::events::{EventBus, PantherEvent};

pub async fn handle_shutdown(event_bus: EventBus) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = std::sync::Mutex::new(Some(tx));

    ctrlc::set_handler(move || {
        if let Ok(mut opt) = tx.lock() {
            if let Some(sender) = opt.take() {
                let _ = sender.send(());
            }
        }
    })
    .ok();

    let _ = rx.await;

    println!("\nPanther shutting down...");
    let _ = event_bus.publish(PantherEvent::ShutdownRequested);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    std::process::exit(0);
}
