//! SQL → DBML import (placeholder; implemented in the importer milestone).
use super::sql::Dialect;

pub fn import(_source: &str, _dialect: Dialect) -> Result<String, String> {
    Err("import not implemented yet".to_string())
}
