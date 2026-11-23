//! The title of the question the Archive tab asks before it queues a file the checks ask about.

use shaiya_inspect::validate::{code, Finding};

/// "The skeleton differs from the file it replaces" when every finding that asks is an animation whose skeleton differs, else "This file
/// looks broken".
pub fn findings_title(findings: &[Finding]) -> &'static str {
    let mut asking = findings.iter().filter(|f| f.asks()).peekable();
    if asking.peek().is_some() && asking.all(|f| f.code == code::ANIM_RIG_DIFFERS) {
        "The skeleton differs from the file it replaces"
    } else {
        "This file looks broken"
    }
}
