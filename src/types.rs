use chrono::{DateTime, Local};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Create,
    Modify,
    Remove,
}

#[derive(Clone)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    pub timestamp: DateTime<Local>,
    pub diff: Option<String>, 
}
