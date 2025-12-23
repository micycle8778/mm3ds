use std::convert::AsRef;
use std::fs;
use std::path::Path;
use std::path;
use std::process::Command;

fn walk_dir(dir: impl AsRef<Path>, extension: &str, f: &impl Fn(&Path)) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_dir(&path, extension, f);
            } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
                f(&path);
            }
        }
    }
}

fn main() {
    walk_dir("gfx", "t3s", &|path| {
        dbg!(path);
        let file_data = fs::read_to_string(path).unwrap();

        // gfx/folder/file.t3s => OUT_DIR/folder/file.t3x
        let output_path = path::absolute(Path::new(&std::env::var("OUT_DIR").unwrap()).join(
                path.with_extension("t3x").strip_prefix("gfx/").unwrap()
        )).unwrap();

        let t3s_dir = path.ancestors().nth(1).unwrap();
        let exit_code = Command::new("tex3ds")
            .current_dir(t3s_dir) // set cwd for tex3ds next to the t3s
            .args(file_data.split_whitespace()) // pass parameters from t3s
            .arg("-o") // output path is gfx_build/filename.t3x
            .arg(output_path)
            .status().unwrap();
        assert!(exit_code.success());

        // let input_file = file_data.lines().last().unwrap();
        // println!("cargo::rerun-if-changed={}", t3s_dir.join(input_file).display());
        // println!("cargo::rerun-if-changed={}", path.display());
    });
}
