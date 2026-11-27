import QtQuick
import QtQuick.Window
import GlobalMenu
import Turbine.Native

Window {
    id: win
    width: Math.max(view.cellWidth * 100, 640)
    height: Math.max(view.cellHeight * 32, 400)
    visible: true
    color: "black"
    title: view.title

    // Turbine's menus in the desktop's global menu bar (hearth/arbor,
    // Plasma): the DOS menu bar stays in the window too.
    GlobalMenuBar {
        id: globalMenu
        window: win
        model: JSON.parse(view.menusJson)
        onTriggered: id => view.command(id)
    }
    // (The menu's address goes on the window's surface, which exists only
    // while the window is shown.)
    onVisibleChanged: if (visible) Qt.callLater(globalMenu.attach)

    Item {
        anchors.fill: parent
        // (A thousandth of a pixel down and right: text never lands exactly
        // on a half pixel at fractional scales.)
        anchors.topMargin: 0.001
        anchors.leftMargin: 0.001
        CellView {
            id: view
            anchors.fill: parent
            focus: true
        }
    }
}
