use crate::ListId;

/// Who can see a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListVisibility {
    Public,
    Private,
}

/// A list the viewer owns or follows, as offered when adding a list column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSummary {
    pub id: ListId,
    pub name: String,
    pub members: u64,
    pub visibility: ListVisibility,
}
