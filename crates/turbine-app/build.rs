use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};

fn main() {
    println!("cargo::rerun-if-env-changed=QMAKE");
    println!("cargo::rerun-if-changed=include");
    println!("cargo::rerun-if-changed=cpp");
    let qmake = std::env::var("QMAKE").unwrap_or_else(|_| "qmake6".into());
    if let Ok(o) = std::process::Command::new(&qmake).args(["-query", "QT_INSTALL_QML"]).output() {
        let qml = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !qml.is_empty() {
            println!("cargo::rustc-env=TURBINE_QT_QML={qml}");
        }
    }
    // Qt's private platform headers (the global menu bar: global_menu.cpp).
    let query = |var: &str| {
        std::process::Command::new(&qmake)
            .args(["-query", var])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let mut private_includes = Vec::new();
    if let (Some(headers), Some(version)) = (query("QT_INSTALL_HEADERS"), query("QT_VERSION")) {
        let libs = query("QT_INSTALL_LIBS").unwrap_or_default();
        for module in ["QtCore", "QtGui"] {
            for dir in [
                std::path::Path::new(&headers).join(module).join(&version),
                std::path::Path::new(&libs).join(format!("{module}.framework")).join("Headers").join(&version),
            ] {
                if dir.exists() {
                    private_includes.push(dir.clone());
                    private_includes.push(dir.join(module));
                }
            }
        }
    }
    let mut builder = CxxQtBuilder::new_qml_module(QmlModule::new("Turbine").qml_files([QmlFile::from("qml/Main.qml")]))
        .crate_include_root(None)
        .include_dir("include");
    for dir in private_includes {
        builder = builder.include_dir(dir);
    }
    builder
        .qt_module("Quick")
        .qt_module("Widgets")
        .cpp_file("include/cell_view.h")
        .cpp_file("cpp/cell_view.cpp")
        .cpp_file("include/global_menu.h")
        .cpp_file("cpp/global_menu.cpp")
        .files(["src/bridge.rs"])
        .build();
}
