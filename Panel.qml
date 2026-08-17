import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// Relaunch panel. Anchored to the bar button. Shells out to the `relaunch`
// engine binary for all work and renders its --json output. No privileged
// actions; everything runs as the user, same as any Hyprland command.
Panel {
  id: root
  moduleName: "io.github.laytonf.relaunch"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null

  // --- state populated from `relaunch list/save --json` ---
  property var entries: []
  property int staggerSeconds: 0
  property string statusText: ""
  property bool busy: false

  function open() {
    root.controller.show()
    refresh()
  }
  function close() { root.controller.hide() }
  function toggle() {
    if (root.opened) root.close()
    else root.open()
  }
  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.hostWidget || root, direction)
    return false
  }

  // Reload the table when the panel opens.
  function refresh() {
    root.busy = true
    listProc.running = true
  }

  function applyResult(text) {
    try {
      const r = JSON.parse(text)
      if (r.ok === false) {
        root.statusText = "Error: " + (r.error || "unknown")
        return
      }
      if (r.entries !== undefined) root.entries = r.entries
      if (r.staggerSeconds !== undefined) root.staggerSeconds = r.staggerSeconds
      if (r.added !== undefined)
        root.statusText = "Saved: " + r.added + " new, " + r.updated + " updated."
    } catch (e) {
      root.statusText = "Bad engine output"
    }
  }

  // `relaunch list --json` — populate the table on open.
  Process {
    id: listProc
    command: ["relaunch", "list", "--json"]
    stdout: StdioCollector {
      onStreamFinished: { root.applyResult(this.text); root.busy = false }
    }
  }

  // `relaunch save --json` — capture current layout + write snippet.
  Process {
    id: saveProc
    command: ["relaunch", "save", "--json"]
    stdout: StdioCollector {
      onStreamFinished: { root.applyResult(this.text); root.busy = false }
    }
  }

  // `relaunch generate` — rebuild snippet from edited config.
  Process {
    id: genProc
    command: ["relaunch", "generate"]
    onExited: function(exitCode) {
      root.busy = false
      root.statusText = exitCode === 0 ? "Snippet regenerated." : "Regenerate failed"
    }
  }

  // `relaunch reload` — hyprctl reload to apply immediately.
  Process {
    id: reloadProc
    command: ["relaunch", "reload"]
    onExited: function(exitCode) {
      root.busy = false
      root.statusText = exitCode === 0 ? "Hyprland reloaded." : "Reload failed"
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(360))
    contentHeight: panel.fittedContentHeight(content.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: content
        width: parent.width
        spacing: Style.space(10)

        // --- Title ---
        Text {
          width: parent.width
          text: "Relaunch"
          color: root.barForeground
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.subtitle
          font.bold: true
        }

        Text {
          width: parent.width
          text: "Save the app layout you have right now. On reboot, these apps relaunch into the same workspaces."
          color: root.barForeground
          opacity: 0.75
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.bodySmall
          wrapMode: Text.WordWrap
        }

        // --- Primary action ---
        Rectangle {
          width: parent.width
          height: Style.space(36)
          radius: Style.space(6)
          color: saveMouse.containsMouse ? root.barForeground : "transparent"
          border.color: root.barForeground
          border.width: 1
          opacity: root.busy ? 0.5 : 1.0

          Text {
            anchors.centerIn: parent
            text: root.busy ? "Working…" : "Save Startup App Workspaces"
            color: saveMouse.containsMouse ? root.barBackground : root.barForeground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
            font.bold: true
          }
          MouseArea {
            id: saveMouse
            anchors.fill: parent
            hoverEnabled: true
            enabled: !root.busy
            onClicked: {
              root.busy = true
              root.statusText = ""
              saveProc.running = true
            }
          }
        }

        // --- Secondary actions row ---
        Row {
          width: parent.width
          spacing: Style.space(8)

          // Regenerate snippet from (possibly hand-edited) config.
          Rectangle {
            width: (parent.width - Style.space(8)) / 2
            height: Style.space(30)
            radius: Style.space(6)
            color: genMouse.containsMouse ? root.barForeground : "transparent"
            border.color: root.barForeground
            border.width: 1
            Text {
              anchors.centerIn: parent
              text: "Regenerate"
              color: genMouse.containsMouse ? root.barBackground : root.barForeground
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.bodySmall
            }
            MouseArea {
              id: genMouse
              anchors.fill: parent
              hoverEnabled: true
              onClicked: { root.busy = true; genProc.running = true }
            }
          }

          // Apply now without a reboot.
          Rectangle {
            width: (parent.width - Style.space(8)) / 2
            height: Style.space(30)
            radius: Style.space(6)
            color: reloadMouse.containsMouse ? root.barForeground : "transparent"
            border.color: root.barForeground
            border.width: 1
            Text {
              anchors.centerIn: parent
              text: "Reload Hyprland"
              color: reloadMouse.containsMouse ? root.barBackground : root.barForeground
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.bodySmall
            }
            MouseArea {
              id: reloadMouse
              anchors.fill: parent
              hoverEnabled: true
              onClicked: { root.busy = true; reloadProc.running = true }
            }
          }
        }

        // --- Divider ---
        Rectangle {
          width: parent.width
          height: 1
          color: root.barForeground
          opacity: 0.2
        }

        // --- Captured apps header ---
        Text {
          width: parent.width
          text: root.entries.length > 0
            ? "Captured apps (" + root.entries.length + ")"
            : "No apps captured yet"
          color: root.barForeground
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.bodySmall
          font.bold: true
        }

        // --- Table of entries ---
        Column {
          width: parent.width
          spacing: Style.space(2)

          Repeater {
            model: root.entries
            delegate: Row {
              width: content.width
              spacing: Style.space(8)

              Text {
                text: "ws " + modelData.workspace
                color: root.barForeground
                opacity: modelData.enabled ? 1.0 : 0.4
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
                width: Style.space(44)
              }
              Text {
                text: modelData.class
                color: root.barForeground
                opacity: modelData.enabled ? 1.0 : 0.4
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
                elide: Text.ElideRight
                width: content.width - Style.space(44) - Style.space(8)
              }
            }
          }
        }

        // --- Status line ---
        Text {
          width: parent.width
          visible: root.statusText.length > 0
          text: root.statusText
          color: root.barForeground
          opacity: 0.75
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.bodySmall
          wrapMode: Text.WordWrap
        }

        Text {
          width: parent.width
          text: "Edit ~/.config/omarchy-relaunch/config.json to fine-tune, then Regenerate."
          color: root.barForeground
          opacity: 0.5
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.bodySmall
          wrapMode: Text.WordWrap
        }
      }
    }
  }
}
