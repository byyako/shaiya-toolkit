fn main() {
    shaiya_build::build_info();
    shaiya_build::windows_resources(&shaiya_build::WinRes {
        file_description: "Shaiya command line",
        original_filename: "shaiya.exe",
        icon: Some("../../assets/icons/shaiya.ico"),
        gui: false,
    });
}
