fn main() {
    shaiya_build::build_info();
    shaiya_build::windows_resources(&shaiya_build::WinRes {
        file_description: "Shaiya SVMap Editor",
        original_filename: "shaiya-svmap-editor.exe",
        icon: Some("../../assets/icons/shaiya-svmap-editor.ico"),
        gui: true,
    });
}
