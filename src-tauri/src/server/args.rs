use super::CommandPreview;
use crate::settings::{Profile, Settings};
use std::path::Path;

pub fn build(profile: &Profile, settings: &Settings, server_exe: &Path) -> CommandPreview {
    let _ = (profile, settings);
    CommandPreview { program: server_exe.display().to_string(), args: vec![], display: String::new() }
}
