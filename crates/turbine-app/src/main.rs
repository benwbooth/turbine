//! Turbine's desktop app (Qt Quick, cxx-qt): the same IDE as the terminal
//! version, its cells painted with QPainter in the desktop's own fonts.

mod bridge;

use cxx_qt_lib::{QQmlApplicationEngine, QString, QUrl};
use std::path::PathBuf;

fn main() {
    // (As a grammar's trial child: try it and exit, before any Qt.)
    turbine::grammars_check_from_args();
    let files: Vec<PathBuf> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).map(PathBuf::from).collect();
    bridge::ffi::create_app();
    bridge::init(&files);
    let mut engine = QQmlApplicationEngine::new();
    if let Some(mut engine) = engine.as_mut() {
        if let Some(qml) = option_env!("TURBINE_QT_QML") {
            if std::path::Path::new(qml).exists() {
                engine.as_mut().add_import_path(&QString::from(qml));
            }
        }
        engine.load(&QUrl::from("qrc:/qt/qml/Turbine/qml/Main.qml"));
    }
    let code = bridge::ffi::exec_app();
    bridge::shutdown();
    std::process::exit(code);
}
