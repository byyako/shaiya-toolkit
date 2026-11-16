fn main() {
    shaiya_build::build_info();
    shaiya_build::windows_resources(&shaiya_build::WinRes {
        file_description: "Shaiya Archive & Patch Studio",
        original_filename: "shaiya-studio.exe",
        icon: Some("../../assets/icons/shaiya-studio.ico"),
        gui: true,
    });
}
