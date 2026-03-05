use uuid::Uuid;

pub struct AgentContext {
    pub session_id: Uuid,
    pub user_input: String,
    pub media_path: Option<String>,
}

impl AgentContext {
    pub fn new(session_id: Uuid, user_input: String, media_path: Option<String>) -> Self {
        Self { session_id, user_input, media_path }
    }
}
