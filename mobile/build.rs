// Die Oberfläche (Dioxus, `mobile/ui/`) wird mit `scripts/mobile.sh` nach `mobile/ui/dist/public`
// gebaut und hier eingebettet: je Datei ein Eintrag in `$OUT_DIR/files.rs`, Text und WebAssembly
// zusätzlich gzip-komprimiert. Fehlt der Ordner, zeigt der Server einen Hinweis.
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=ui/dist/public");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/dist/public");
    let mut files = Vec::new();
    if root.join("index.html").is_file() {
        collect(&root, &root, &mut files);
    }
    files.sort();
    let mut code = String::from("pub static FILES: &[StaticFile] = &[\n");
    for (i, rel) in files.iter().enumerate() {
        let src = root.join(rel);
        println!("cargo:rerun-if-changed={}", src.display());
        let data = std::fs::read(&src).unwrap();
        let gz = if [".html", ".js", ".css", ".wasm", ".svg", ".json", ".webmanifest"].iter().any(|e| rel.ends_with(e)) {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            enc.write_all(&data).unwrap();
            let path = out.join(format!("f-{i}.gz"));
            std::fs::write(&path, enc.finish().unwrap()).unwrap();
            format!("Some(include_bytes!({:?}))", path.display().to_string())
        } else {
            "None".into()
        };
        code += &format!("    StaticFile {{ path: {rel:?}, body: include_bytes!({:?}), gzip: {gz} }},\n", src.display().to_string());
    }
    code += "];\n";
    std::fs::write(out.join("files.rs"), code).unwrap();
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out);
        } else {
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
}
