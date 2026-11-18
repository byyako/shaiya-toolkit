fn main() {
    shaiya_build::build_info();
    shaiya_build::windows_resources(&shaiya_build::WinRes {
        file_description: "Shaiya Toolkit",
        original_filename: "shaiya-toolkit.exe",
        icon: Some("../../assets/icons/shaiya-toolkit.ico"),
        gui: true,
    });
}
