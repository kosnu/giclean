//! Default text rendering. Inspection code does not construct CLI rows.
use std::fmt;

pub fn candidate(target: impl fmt::Display) -> String {
    format!("candidate: {target}")
}
pub fn deleted(target: impl fmt::Display) -> String {
    format!("{target}: deleted")
}
pub fn failed(target: impl fmt::Display, detail: impl fmt::Display) -> String {
    format!("{target}: failed ({detail})")
}
