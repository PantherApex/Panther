use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use lettre::message::{header::ContentType, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as LettreMessage, Tokio1Executor};
use shared::bus::{InboundMessage, MessageBus};
use shared::channel::{Channel, FileKind};
use shared::errors::{PantherError, PantherResult};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio_native_tls;

pub const EMAIL_CHANNEL: &str = "email";

#[derive(Clone)]
pub struct EmailConfig {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_username: String,
    pub imap_password: String,
    pub imap_mailbox: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub from_address: String,
    pub allow_from: Vec<String>,
    pub poll_interval_secs: u64,
    pub max_body_chars: usize,
}

struct PendingReply {
    to: String,
    subject: String,
    in_reply_to: Option<String>,
}

pub struct EmailChannel {
    config: EmailConfig,
    allow_set: Arc<std::collections::HashSet<String>>,
    bus: MessageBus,
    last_uid: Arc<RwLock<Option<u32>>>,
    reply_map: Arc<RwLock<HashMap<String, PendingReply>>>,
}

impl EmailChannel {
    pub fn new(config: EmailConfig, bus: MessageBus) -> Self {
        let allow_set = Arc::new(config.allow_from.iter().cloned().collect());
        Self {
            config,
            allow_set,
            bus,
            last_uid: Arc::new(RwLock::new(None)),
            reply_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn is_allowed(&self, addr: &str) -> bool {
        self.allow_set.is_empty() || self.allow_set.contains(addr)
    }

    pub async fn run_loop(self: Arc<Self>) {
        let interval = Duration::from_secs(self.config.poll_interval_secs.max(10));
        loop {
            if let Err(e) = self.poll_once().await {
                eprintln!("[panther:email] poll error: {}", e);
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn poll_once(&self) -> PantherResult<()> {
        let last_uid = *self.last_uid.read().await;
        let emails = imap_fetch_unseen(
            &self.config.imap_host,
            self.config.imap_port,
            &self.config.imap_username,
            &self.config.imap_password,
            &self.config.imap_mailbox,
            last_uid,
        ).await?;

        for email in emails {
            if !self.is_allowed(&email.from_addr) {
                continue;
            }

            let body = if email.body.len() > self.config.max_body_chars {
                format!("{}\n\n[...truncated]", &email.body[..self.config.max_body_chars])
            } else {
                email.body.clone()
            };

            let chat_id = sanitize_for_key(&email.from_addr);
            let session_key = format!("{}:{}", EMAIL_CHANNEL, chat_id);

            {
                let mut map = self.reply_map.write().await;
                map.insert(session_key.clone(), PendingReply {
                    to: email.from_addr.clone(),
                    subject: email.subject.clone(),
                    in_reply_to: email.message_id.clone(),
                });
            }

            *self.last_uid.write().await = Some(email.uid);

            let inbound = InboundMessage {
                channel: EMAIL_CHANNEL.to_string(),
                sender_id: email.from_addr.clone(),
                chat_id,
                content: format!("Subject: {}\n\n{}", email.subject, body),
                media_path: None,
                image_b64: None,
                session_key_override: Some(session_key),
            };

            self.bus.publish_inbound(inbound).await;
        }

        Ok(())
    }

    fn build_smtp(&self) -> PantherResult<AsyncSmtpTransport<Tokio1Executor>> {
        let creds = Credentials::new(
            self.config.smtp_username.clone(),
            self.config.smtp_password.clone(),
        );
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.smtp_host)
            .map_err(|e| PantherError::ChannelError(format!("SMTP relay build failed: {}", e)))?
            .port(self.config.smtp_port)
            .credentials(creds)
            .build();
        Ok(transport)
    }
}

fn sanitize_for_key(addr: &str) -> String {
    addr.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

struct FetchedEmail {
    uid: u32,
    from_addr: String,
    subject: String,
    body: String,
    message_id: Option<String>,
}

async fn imap_fetch_unseen(
    host: &str,
    port: u16,
    user: &str,
    pass: &str,
    mailbox: &str,
    last_uid: Option<u32>,
) -> PantherResult<Vec<FetchedEmail>> {
    let tcp = TcpStream::connect(format!("{}:{}", host, port)).await
        .map_err(|e| PantherError::ChannelError(format!("IMAP TCP connect: {}", e)))?;

    let cx = native_tls::TlsConnector::new()
        .map_err(|e| PantherError::ChannelError(format!("IMAP TLS init: {}", e)))?;

    let tls_stream = tokio_native_tls::TlsConnector::from(cx)
        .connect(host, tcp)
        .await
        .map_err(|e| PantherError::ChannelError(format!("IMAP TLS handshake: {}", e)))?;

    let (read_half, mut write_half) = tokio::io::split(tls_stream);
    let mut reader = BufReader::new(read_half);
    let mut tag_n: u32 = 0;

    async fn read_response(
        reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
        tag: u32,
    ) -> PantherResult<Vec<String>> {
        let tag_ok = format!("A{:04} OK", tag);
        let tag_no = format!("A{:04} NO", tag);
        let tag_bad = format!("A{:04} BAD", tag);
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await
                .map_err(|e| PantherError::ChannelError(format!("IMAP read: {}", e)))?;
            let trimmed = line.trim_end().to_string();
            if trimmed.starts_with(&tag_ok) { break; }
            if trimmed.starts_with(&tag_no) || trimmed.starts_with(&tag_bad) {
                return Err(PantherError::ChannelError(format!("IMAP error: {}", trimmed)));
            }
            lines.push(trimmed);
        }
        Ok(lines)
    }

    macro_rules! imap_cmd {
        ($fmt:expr $(, $arg:expr)*) => {{
            tag_n += 1;
            let line = format!(concat!("A{:04} ", $fmt, "\r\n"), tag_n $(, $arg)*);
            write_half.write_all(line.as_bytes()).await
                .map_err(|e| PantherError::ChannelError(format!("IMAP write: {}", e)))?;
            tag_n
        }};
    }

    let mut greeting = String::new();
    reader.read_line(&mut greeting).await
        .map_err(|e| PantherError::ChannelError(format!("IMAP greeting: {}", e)))?;
    if !greeting.contains("* OK") {
        return Err(PantherError::ChannelError(format!("IMAP unexpected greeting: {}", greeting.trim())));
    }

    let t = imap_cmd!("LOGIN \"{}\" \"{}\"", escape_imap(user), escape_imap(pass));
    read_response(&mut reader, t).await.map_err(|_| PantherError::ChannelError("IMAP LOGIN failed — check credentials".to_string()))?;

    let t = imap_cmd!("SELECT \"{}\"", mailbox);
    read_response(&mut reader, t).await?;

    let search_query = match last_uid {
        Some(uid) => format!("UID SEARCH UID {}:* UNSEEN", uid + 1),
        None => "UID SEARCH UNSEEN".to_string(),
    };
    let t = imap_cmd!("{}", search_query);
    let search_lines = read_response(&mut reader, t).await?;

    let uids: Vec<u32> = search_lines.iter()
        .filter(|l| l.starts_with("* SEARCH"))
        .flat_map(|l| l["* SEARCH".len()..].split_whitespace().filter_map(|s| s.parse::<u32>().ok()))
        .collect();

    if uids.is_empty() {
        let t = imap_cmd!("LOGOUT");
        let _ = read_response(&mut reader, t).await;
        return Ok(Vec::new());
    }

    let uid_set = uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
    let t = imap_cmd!("UID FETCH {} (UID RFC822)", uid_set);

    let mut raw_messages: Vec<(u32, Vec<u8>)> = Vec::new();
    let tag_ok = format!("A{:04} OK", t);
    let tag_no = format!("A{:04} NO", t);
    let tag_bad = format!("A{:04} BAD", t);
    let mut current_uid: Option<u32> = None;
    let mut bytes_remaining: usize = 0;
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await
            .map_err(|e| PantherError::ChannelError(format!("IMAP fetch read: {}", e)))?;
        let trimmed = line.trim_end();

        if trimmed.starts_with(&tag_ok) || trimmed.starts_with(&tag_no) || trimmed.starts_with(&tag_bad) {
            if let (Some(uid), data) = (current_uid.take(), std::mem::take(&mut buffer)) {
                if !data.is_empty() { raw_messages.push((uid, data)); }
            }
            break;
        }

        if bytes_remaining > 0 {
            let bytes = line.as_bytes();
            let take = bytes.len().min(bytes_remaining);
            buffer.extend_from_slice(&bytes[..take]);
            bytes_remaining -= take;
            if bytes_remaining == 0 {
                if let Some(uid) = current_uid.take() {
                    raw_messages.push((uid, std::mem::take(&mut buffer)));
                }
            }
            continue;
        }

        if trimmed.starts_with("* ") && trimmed.contains("FETCH") {
            if let Some(uid) = parse_uid_from_fetch(trimmed) {
                current_uid = Some(uid);
            }
            if let Some(size) = parse_literal_size(trimmed) {
                bytes_remaining = size;
                buffer.clear();
            }
        }
    }

    let t = imap_cmd!("LOGOUT");
    let _ = read_response(&mut reader, t).await;

    Ok(raw_messages.into_iter().filter_map(|(uid, raw)| parse_email(uid, &raw)).collect())
}

fn escape_imap(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_uid_from_fetch(line: &str) -> Option<u32> {
    let upper = line.to_uppercase();
    let pos = upper.find("UID ")?;
    line[pos + 4..].split_whitespace().next()?.parse().ok()
}

fn parse_literal_size(line: &str) -> Option<usize> {
    let open = line.rfind('{')?;
    let close = line.rfind('}')?;
    if close > open {
        line[open + 1..close].parse::<usize>().ok()
    } else {
        None
    }
}

fn parse_email(uid: u32, raw: &[u8]) -> Option<FetchedEmail> {
    let parsed = mail_parser::MessageParser::default().parse(raw)?;

    let from_addr = parsed.from()
        .and_then(|al| al.first())
        .and_then(|a| a.address())
        .unwrap_or("")
        .to_string();

    if from_addr.is_empty() { return None; }

    let subject = parsed.subject().unwrap_or("(no subject)").to_string();
    let message_id = parsed.message_id().map(|s| s.to_string());

    let body = if let Some(html) = parsed.body_html(0) {
        html2text::from_read(html.as_bytes(), 120)
    } else {
        parsed.body_text(0).unwrap_or_default().to_string()
    };

    Some(FetchedEmail { uid, from_addr, subject, body, message_id })
}

impl Channel for EmailChannel {
    fn name(&self) -> &str { EMAIL_CHANNEL }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let session_key = format!("{}:{}", EMAIL_CHANNEL, chat_id);
            let reply_info = {
                let map = self.reply_map.read().await;
                map.get(&session_key).map(|r| (r.to.clone(), r.subject.clone(), r.in_reply_to.clone()))
            };

            let (to_addr, subject, in_reply_to) = match reply_info {
                Some(r) => r,
                None => { eprintln!("[panther:email] no reply context for {}", chat_id); return Ok(()); }
            };

            let from_mb: Mailbox = self.config.from_address.parse()
                .map_err(|e| PantherError::ChannelError(format!("Invalid from: {}", e)))?;
            let to_mb: Mailbox = to_addr.parse()
                .map_err(|e| PantherError::ChannelError(format!("Invalid to: {}", e)))?;

            let reply_subject = if subject.to_lowercase().starts_with("re:") {
                subject
            } else {
                format!("Re: {}", subject)
            };

            let mut builder = LettreMessage::builder().from(from_mb).to(to_mb).subject(reply_subject);
            if let Some(mid) = in_reply_to { builder = builder.in_reply_to(mid); }

            let email = builder.body(text.to_string())
                .map_err(|e| PantherError::ChannelError(format!("Email build: {}", e)))?;

            self.build_smtp()?.send(email).await
                .map_err(|e| PantherError::ChannelError(format!("SMTP send: {}", e)))?;

            Ok(())
        })
    }

    fn send_file<'a>(
        &'a self,
        chat_id: &'a str,
        path: &'a PathBuf,
        _kind: FileKind,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let session_key = format!("{}:{}", EMAIL_CHANNEL, chat_id);
            let reply_info = {
                let map = self.reply_map.read().await;
                map.get(&session_key).map(|r| (r.to.clone(), r.subject.clone(), r.in_reply_to.clone()))
            };

            let (to_addr, subject, in_reply_to) = match reply_info {
                Some(r) => r,
                None => { eprintln!("[panther:email] no reply context for file to {}", chat_id); return Ok(()); }
            };

            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "attachment".to_string());

            let bytes = tokio::fs::read(path).await
                .map_err(|e| PantherError::ChannelError(format!("File read: {}", e)))?;

            let from_mb: Mailbox = self.config.from_address.parse()
                .map_err(|e| PantherError::ChannelError(format!("Invalid from: {}", e)))?;
            let to_mb: Mailbox = to_addr.parse()
                .map_err(|e| PantherError::ChannelError(format!("Invalid to: {}", e)))?;

            let reply_subject = if subject.to_lowercase().starts_with("re:") { subject } else { format!("Re: {}", subject) };

            let mut builder = LettreMessage::builder().from(from_mb).to(to_mb).subject(reply_subject);
            if let Some(mid) = in_reply_to { builder = builder.in_reply_to(mid); }

            let ct = ContentType::parse("application/octet-stream").unwrap();
            let attachment_part = lettre::message::Attachment::new(filename).body(bytes, ct);
            let email = builder
                .multipart(
                    MultiPart::mixed()
                        .singlepart(SinglePart::plain("[see attachment]".to_string()))
                        .singlepart(attachment_part)
                )
                .map_err(|e| PantherError::ChannelError(format!("Email build: {}", e)))?;

            self.build_smtp()?.send(email).await
                .map_err(|e| PantherError::ChannelError(format!("SMTP send: {}", e)))?;

            Ok(())
        })
    }
}
