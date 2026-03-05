use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct SkillStore {
    #[allow(dead_code)]
    path: PathBuf,
}

impl SkillStore {
    pub fn new(base: &Path) -> Self {
        Self { path: base.join("skills.json") }
    }
}
