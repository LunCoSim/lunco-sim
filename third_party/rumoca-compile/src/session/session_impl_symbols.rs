use super::*;

pub(crate) fn workspace_symbol_query_match_score(name: &str, query: &str) -> u8 {
    let name_lower = name.to_lowercase();
    if name_lower == query {
        0
    } else if name_lower.starts_with(query) {
        1
    } else {
        2
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(SessionConfig::default())
    }
}
