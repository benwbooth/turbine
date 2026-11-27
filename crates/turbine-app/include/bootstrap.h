// The Qt application for Turbine's desktop app.
#pragma once

#include <QtQml/QQmlEngine>
#include <QtQuick/QQuickWindow>
#include <QtWidgets/QApplication>

#include "cell_view.h"
#include "global_menu.h"

namespace turbine {

// QApplication (Qt.labs.platform's native menus need widgets), native text
// rendering for anything Qt Quick draws itself, and the cell view for QML.
inline void create_app()
{
    QQuickWindow::setTextRenderType(QQuickWindow::NativeTextRendering);
    static int argc = 1;
    static char name[] = "turbine";
    static char* argv[] = { name, nullptr };
    QApplication::setApplicationName(QStringLiteral("Turbine"));
    QApplication::setOrganizationName(QStringLiteral("Turbine"));
    QApplication::setDesktopFileName(QStringLiteral("turbine"));
    new QApplication(argc, argv);
    qmlRegisterType<CellView>("Turbine.Native", 1, 0, "CellView");
    qmlRegisterType<GlobalMenuBar>("GlobalMenu", 1, 0, "GlobalMenuBar");
}

inline int exec_app() { return QApplication::exec(); }

} // namespace turbine
