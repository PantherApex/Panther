use std::sync::Arc;
use serde_json::Value;
use shared::types::ToolDefinition;
use crate::file_tracker::TurnFileTracker;
use super::Tool;
use super::capture::CaptureMediaTool;
use super::message::MessageTool;
use super::cron::CronTool;
use super::spawn::SpawnTool;
use super::send_file::SendFileTool;

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    message_tool: Option<Arc<MessageTool>>,
    cron_tool: Option<Arc<CronTool>>,
    spawn_tool: Option<Arc<SpawnTool>>,
    send_file_tool: Option<Arc<SendFileTool>>,
    capture_tool: Option<Arc<CaptureMediaTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            message_tool: None,
            cron_tool: None,
            spawn_tool: None,
            send_file_tool: None,
            capture_tool: None,
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn register_capture_tool(&mut self, tool: Arc<CaptureMediaTool>) {
        self.capture_tool = Some(Arc::clone(&tool));
        self.tools.push(Box::new(CaptureMediaToolWrapper(tool)));
    }

    pub fn register_message_tool(&mut self, tool: Arc<MessageTool>) {
        self.message_tool = Some(Arc::clone(&tool));
        self.tools.push(Box::new(MessageToolWrapper(tool)));
    }

    pub fn register_cron_tool(&mut self, tool: Arc<CronTool>) {
        self.cron_tool = Some(Arc::clone(&tool));
        self.tools.push(Box::new(CronToolWrapper(tool)));
    }

    pub fn register_spawn_tool(&mut self, tool: Arc<SpawnTool>) {
        self.spawn_tool = Some(Arc::clone(&tool));
        self.tools.push(Box::new(SpawnToolWrapper(tool)));
    }

    pub fn register_send_file_tool(&mut self, tool: Arc<SendFileTool>) {
        self.send_file_tool = Some(Arc::clone(&tool));
        self.tools.push(Box::new(SendFileToolWrapper(tool)));
    }

    pub fn get_message_tool(&self) -> Option<&MessageTool> { self.message_tool.as_deref() }
    pub fn get_cron_tool(&self) -> Option<&CronTool> { self.cron_tool.as_deref() }
    pub fn get_spawn_tool(&self) -> Option<&SpawnTool> { self.spawn_tool.as_deref() }
    pub fn get_send_file_tool(&self) -> Option<&SendFileTool> { self.send_file_tool.as_deref() }

    pub async fn set_turn_file_tracker(&self, tracker: Arc<TurnFileTracker>) {
        if let Some(ref tool) = self.send_file_tool {
            tool.set_turn_tracker(tracker).await;
        }
    }

    pub async fn reset_capture_turn_cache(&self) {
        if let Some(ref tool) = self.capture_tool {
            tool.reset_turn().await;
        }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| ToolDefinition {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters(),
        }).collect()
    }

    pub async fn execute(&self, name: &str, args: Value) -> String {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.execute(args).await,
            None => format!("Tool not found: {}", name),
        }
    }
}

macro_rules! tool_wrapper {
    ($wrapper:ident, $inner:ty) => {
        struct $wrapper(Arc<$inner>);
        impl Tool for $wrapper {
            fn name(&self) -> &str { self.0.name() }
            fn description(&self) -> &str { self.0.description() }
            fn parameters(&self) -> Value { self.0.parameters() }
            fn execute<'a>(&'a self, args: Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
                self.0.execute(args)
            }
        }
    };
}

tool_wrapper!(MessageToolWrapper, MessageTool);
tool_wrapper!(CronToolWrapper, CronTool);
tool_wrapper!(SpawnToolWrapper, SpawnTool);
tool_wrapper!(SendFileToolWrapper, SendFileTool);
tool_wrapper!(CaptureMediaToolWrapper, CaptureMediaTool);
