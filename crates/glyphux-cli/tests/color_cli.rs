use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn invoke(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_glyphux"))
        .args(args)
        .output()
        .expect("run CLI")
}
fn font(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../glyphux/tests/assets")
        .join(name)
}

#[test]
fn bakes_colr_fonts_and_rejects_invalid_requests() {
    let output = std::env::temp_dir().join(format!("glyphux-cli-bake-{}", std::process::id()));
    for (file, text) in [("Noto-COLRv1-Subset.ttf", "🔥"), ("Twemoji-Subset.ttf", "🤖\u{fe0f}")] {
        let path = font(file);
        let result = invoke(&[
            "color",
            "--font",
            path.to_str().unwrap(),
            "--text",
            text,
            "--resolution",
            "128",
            "--base-resolution",
            "32",
            "--page-size",
            "256",
            "-o",
            output.to_str().unwrap(),
        ]);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let manifest = std::fs::read_to_string(output.join("atlas.json")).unwrap();
        assert!(manifest.contains("glyphux-color-atlas"));
        for name in [
            "tiles.bin",
            "paints.bin",
            "stops.bin",
            "recipes.bin",
            "config.bin",
            "color.wgsl",
        ] {
            assert_ne!(std::fs::read(output.join(name)).unwrap().len(), 0);
        }
        assert_eq!(std::fs::read(output.join("atlas-0.rgba")).unwrap().len(), 256 * 256 * 4);
        let png = std::fs::read(output.join("atlas-0.png")).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 256);
        assert_eq!(png[25], 6);
        std::fs::remove_dir_all(&output).unwrap();
    }
    let path = font("Noto-COLRv1-Subset.ttf");
    let base = [
        "color",
        "--font",
        path.to_str().unwrap(),
        "--text",
        "🔥",
        "-o",
        output.to_str().unwrap(),
    ];
    for options in [
        vec!["--palette", "65535"],
        vec!["--size", "0"],
        vec!["--size", "8"],
        vec!["--range", "NaN"],
        vec!["--base-resolution", "31"],
        vec!["--foreground", "invalid"],
        vec!["--glyph-index", "1"],
        vec!["--fontvar", "wght=NaN"],
        vec!["--max-grid", "256"],
        vec!["--background", "ffffff"],
    ] {
        let args: Vec<_> = base.iter().copied().chain(options).collect();
        let result = invoke(&args);
        assert!(!result.status.success(), "accepted {args:?}");
        assert!(!output.exists());
    }
    for text in ["", "🔥🤖", "missing"] {
        assert!(
            !invoke(&[
                "color",
                "--font",
                path.to_str().unwrap(),
                "--text",
                text,
                "-o",
                output.to_str().unwrap()
            ])
            .status
            .success()
        );
        assert!(!output.exists());
    }
}

#[test]
fn atlas_command_bakes_multiple_glyphs_and_deduplicates_selection() {
    let output = std::env::temp_dir().join(format!("glyphux-multi-bake-{}", std::process::id()));
    let path = font("Noto-COLRv1-Subset.ttf");
    let result = Command::new(env!("CARGO_BIN_EXE_glyphux-atlas"))
        .args([
            "color",
            "--font",
            path.to_str().unwrap(),
            "--text",
            "🔥",
            "--text",
            "😀",
            "--text",
            "🔥",
            "--resolution",
            "256",
            "--page-size",
            "256",
            "-o",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let manifest = std::fs::read_to_string(output.join("atlas.json")).unwrap();
    assert_eq!(manifest.matches("glyphId").count(), 2);
    assert!(manifest.contains("atlas-1.rgba"));
    std::fs::remove_dir_all(&output).unwrap();
}

#[test]
fn old_modes_and_new_help_remain_available() {
    for args in [
        vec!["--help"],
        vec!["color", "--help"],
        vec!["metrics", "-defineshape", "{ 0,0; 0,1; 1,1; 1,0; # }"],
    ] {
        let result = invoke(&args);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
    assert!(!invoke(&["color"]).status.success());
    assert!(!invoke(&["color", "--font", "missing.ttf"]).status.success());
}
