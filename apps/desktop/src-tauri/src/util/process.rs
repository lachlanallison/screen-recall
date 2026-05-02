/// Process spawning helpers.
///
/// On Windows, GUI apps can still spawn child console windows unless we set
/// `CREATE_NO_WINDOW`. We apply this for OCR + managed server child processes.
pub fn configure_hidden_std(cmd: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

pub fn configure_hidden_tokio(cmd: &mut tokio::process::Command) {
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}
