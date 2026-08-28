//! The shipped identity assets: if any of these fail, Notification Center and
//! the Linux banner show a generic blob again.

#[test]
fn desktop_entry_names_the_icon() {
    let desktop = include_str!("../assets/animesh.desktop");
    assert!(
        desktop.contains("Icon=animesh\n"),
        "animesh.desktop has no Icon=animesh"
    );
}

#[test]
fn app_icon_is_an_icns() {
    let icns = include_bytes!("../assets/AppIcon.icns");
    assert!(icns.len() > 1024, "AppIcon.icns is too small to be real");
    assert_eq!(&icns[..4], b"icns", "AppIcon.icns is not an icns file");
}

#[test]
fn menubar_template_is_a_pdf() {
    let pdf = include_bytes!("../assets/icons/menubar-template.pdf");
    assert_eq!(&pdf[..5], b"%PDF-", "menubar template is not a PDF");
}

#[test]
fn hicolor_pngs_are_present() {
    for (size, bytes) in [
        (
            "16x16",
            include_bytes!("../assets/icons/hicolor/16x16/apps/animesh.png").as_slice(),
        ),
        (
            "24x24",
            include_bytes!("../assets/icons/hicolor/24x24/apps/animesh.png").as_slice(),
        ),
        (
            "32x32",
            include_bytes!("../assets/icons/hicolor/32x32/apps/animesh.png").as_slice(),
        ),
        (
            "48x48",
            include_bytes!("../assets/icons/hicolor/48x48/apps/animesh.png").as_slice(),
        ),
        (
            "256x256",
            include_bytes!("../assets/icons/hicolor/256x256/apps/animesh.png").as_slice(),
        ),
        (
            "512x512",
            include_bytes!("../assets/icons/hicolor/512x512/apps/animesh.png").as_slice(),
        ),
    ] {
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{size} is not a PNG");
    }
}
