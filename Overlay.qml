import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import qs.Commons
import qs.Ui

// Fullscreen log overlay. Same contract as omarchy.emojis / omarchy.clipboard:
// Item + opened/open/close/dismiss/toggle, PanelWindow on the overlay layer,
// dimmed scrim, centered card. There is no Overlay {} type in the shell.
Item {
  id: root

  property var shell: null
  property var manifest: null

  property bool opened: false
  property string logPath: Quickshell.env("HOME") + "/.config/omarchy-relaunch/last-boot.log"
  property string logBody: ""
  property string emptyHint: "No last-boot log yet. Reboot, or run: relaunch boot"
  property bool logMissing: false
  readonly property int maxChars: 65536

  property color background: Color.menu.background
  property color foreground: Color.menu.text
  property color border: Color.menu.border
  property var borderSpec: Border.surfaceSpec("menu", "border", border, Math.max(1, Style.space(2)))
  property color scrim: Color.menu.scrim
  readonly property int cornerRadius: Style.cornerRadius
  property string fontFamily: Style.font.family
  property int contentMargin: Style.spacing.panelPadding
  property int headerHeight: Math.max(Style.space(34), Style.font.title + Style.spacing.controlPaddingY * 2)
  property int contentSpacing: Style.spacing.md
  property int cardWidth: Math.min(panel.width * 0.78, panel.width - Style.gapsOut * 2)
  property int cardHeight: Math.min(panel.height * 0.82, panel.height - Style.gapsOut * 2)

  function open(payloadJson) {
    var payload = ({})
    try { payload = JSON.parse(payloadJson || "{}") } catch (e) { payload = ({}) }
    if (payload.logPath)
      root.logPath = payload.logPath
    root.opened = true
    logFile.reload()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function close() {
    root.opened = false
  }

  function dismiss() {
    root.opened = false
    if (root.shell && typeof root.shell.hide === "function")
      root.shell.hide((root.manifest && root.manifest.id) || "io.github.laytonf.relaunch")
  }

  function toggle() {
    if (root.opened) root.dismiss()
    else root.open("{}")
  }

  function applyText(raw) {
    var s = String(raw || "")
    if (s.length > root.maxChars)
      s = "… (truncated to last 64 KiB)\n" + s.slice(s.length - root.maxChars)
    root.logBody = s
    root.logMissing = false
    Qt.callLater(root.scrollToEnd)
  }

  function applyMissing() {
    root.logBody = ""
    root.logMissing = true
  }

  function scrollToEnd() {
    if (!scroller)
      return
    scroller.contentX = 0
    scroller.contentY = Math.max(0, scroller.contentHeight - scroller.height)
  }

  function scrollBy(dx, dy) {
    scroller.contentX = Math.max(0, Math.min(scroller.contentX + dx, Math.max(0, scroller.contentWidth - scroller.width)))
    scroller.contentY = Math.max(0, Math.min(scroller.contentY + dy, Math.max(0, scroller.contentHeight - scroller.height)))
  }

  function scrollPage(direction) {
    root.scrollBy(0, direction * Math.max(Style.space(48), scroller.height * 0.85))
  }

  FileView {
    id: logFile
    path: root.logPath
    watchChanges: true
    printErrors: false
    onLoaded: root.applyText(text())
    onLoadFailed: root.applyMissing()
    onFileChanged: reload()
  }

  PanelWindow {
    id: panel
    visible: root.opened
    anchors { top: true; bottom: true; left: true; right: true }
    color: "transparent"
    WlrLayershell.namespace: "omarchy-relaunch"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    exclusionMode: ExclusionMode.Ignore

    Rectangle {
      anchors.fill: parent
      color: root.scrim
    }

    MouseArea {
      anchors.fill: parent
      onClicked: root.dismiss()
    }

    BorderSurface {
      id: card
      width: root.cardWidth
      height: root.cardHeight
      radius: root.cornerRadius
      anchors.centerIn: parent
      color: root.background
      borderSpec: root.borderSpec
      padding: root.contentMargin

      MouseArea { anchors.fill: parent; onClicked: {} }

      Item {
        id: keyCatcher
        anchors.fill: parent
        focus: true

        Keys.priority: Keys.BeforeItem
        Keys.onPressed: function(event) {
          var line = Math.max(Style.space(16), Style.font.bodySmall + Style.space(4))
          if (event.key === Qt.Key_Escape) {
            root.dismiss()
            event.accepted = true
          } else if (event.key === Qt.Key_Up || event.text === "k") {
            root.scrollBy(0, -line)
            event.accepted = true
          } else if (event.key === Qt.Key_Down || event.text === "j") {
            root.scrollBy(0, line)
            event.accepted = true
          } else if (event.key === Qt.Key_Left || event.text === "h") {
            root.scrollBy(-Style.space(48), 0)
            event.accepted = true
          } else if (event.key === Qt.Key_Right || event.text === "l") {
            root.scrollBy(Style.space(48), 0)
            event.accepted = true
          } else if (event.key === Qt.Key_PageUp) {
            root.scrollPage(-1)
            event.accepted = true
          } else if (event.key === Qt.Key_PageDown || event.key === Qt.Key_Space) {
            root.scrollPage(1)
            event.accepted = true
          } else if (event.key === Qt.Key_Home) {
            scroller.contentY = 0
            event.accepted = true
          } else if (event.key === Qt.Key_End) {
            root.scrollToEnd()
            event.accepted = true
          }
        }
      }

      Column {
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset
        spacing: root.contentSpacing

        Item {
          width: parent.width
          height: root.headerHeight

          Text {
            anchors.left: parent.left
            anchors.right: closeHint.left
            anchors.rightMargin: Style.space(12)
            anchors.verticalCenter: parent.verticalCenter
            text: "Last boot log"
            color: root.foreground
            font.family: root.fontFamily
            font.pixelSize: Style.font.heading
            elide: Text.ElideRight
          }

          Text {
            id: closeHint
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            text: "Esc"
            color: root.foreground
            opacity: 0.58
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }
        }

        Flickable {
          id: scroller
          width: parent.width
          height: parent.height - root.headerHeight - root.contentSpacing
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          flickableDirection: Flickable.HorizontalAndVerticalFlick
          contentWidth: Math.max(width, logText.implicitWidth)
          contentHeight: Math.max(height, logText.implicitHeight)

          Text {
            id: logText
            text: root.logMissing ? root.emptyHint : (root.logBody || "Loading…")
            color: root.foreground
            opacity: root.logMissing || !root.logBody ? 0.7 : 1
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.NoWrap
            textFormat: Text.PlainText
          }
        }
      }
    }
  }
}
