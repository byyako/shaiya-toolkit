fn main() {
    shaiya_build::build_info();
    shaiya_build::windows_resources(&shaiya_build::WinRes {
        file_description: "Shaiya Workbench",
        original_filename: "shaiya-workbench.exe",
        icon: Some("../../assets/icons/shaiya-workbench.ico"),
        gui: true,
    });
}
