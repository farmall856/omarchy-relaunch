import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// Relaunch panel. Shells out to `relaunch` for inventory, list edits, and
// boot-policy changes. Existing Hyprland startup apps are shown so they can
// be imported, left alone (ignored), or have their startup line removed.
Panel {
  id: root
  moduleName: "io.github.laytonf.relaunch"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null

  property var entries: []
  property var rows: []
  property var boot: ({ disabled: false, skipOnce: false, active: true })
  property var lastBoot: null
  property int staggerSeconds: 0
  property string statusText: ""
  property string pendingStatus: ""
  property bool busy: false
  property bool confirmRemove: false

  // Plugin-manager clones land the script next to this file, not on PATH.
  readonly property string enginePath: {
    var s = Qt.resolvedUrl("relaunch").toString()
    if (s.indexOf("file://") === 0)
      return decodeURIComponent(s.slice(7))
    return "relaunch"
  }

  readonly property var runningRows: rows.filter(function(r) { return r.kind === "running" })
  readonly property var relaunchRows: rows.filter(function(r) { return r.inRelaunch }).sort(function(a, b) {
    if (a.workspace !== b.workspace) return a.workspace - b.workspace
    return String(a.label).localeCompare(String(b.label))
  })
  readonly property var startupRows: rows.filter(function(r) { return r.kind === "startup" })
  readonly property var ignoredRows: rows.filter(function(r) { return r.kind === "ignored" })

  function open() {
    root.confirmRemove = false
    root.controller.show()
    refresh()
  }
  function close() {
    root.confirmRemove = false
    root.controller.hide()
  }
  function toggle() {
    if (root.opened) root.close()
    else root.open()
  }
  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.hostWidget || root, direction)
    return false
  }

  function refresh() {
    root.busy = true
    run(["list", "--json"])
  }

  function run(args, okText) {
    root.confirmRemove = false
    root.pendingStatus = okText || ""
    root.statusText = ""
    root.busy = true
    actionProc.command = [root.enginePath].concat(args)
    actionProc.running = true
  }

  Component.onCompleted: root.run(["list", "--json"])

  Timer {
    id: disarmRemove
    interval: 4000
    onTriggered: root.confirmRemove = false
  }

  function applyResult(text) {
    try {
      const r = JSON.parse(text)
      if (r.ok === false) {
        root.statusText = "Error: " + (r.error || "unknown")
        return
      }
      if (r.uninstalled === true) {
        root.statusText = "Relaunch removed."
        root.rows = []
        root.entries = []
        root.close()
        Quickshell.execDetached(["omarchy-notification-send", "Relaunch", "Removed from this system."])
        return
      }
      if (r.entries !== undefined) root.entries = r.entries
      if (r.rows !== undefined) root.rows = r.rows
      if (r.boot !== undefined) root.boot = r.boot
      if (r.lastBoot !== undefined) root.lastBoot = r.lastBoot
      if (r.staggerSeconds !== undefined) root.staggerSeconds = r.staggerSeconds
      if (r.added !== undefined)
        root.statusText = "Saved: " + r.added + " new, " + r.updated + " updated."
    } catch (e) {
      root.statusText = "Bad engine output"
    }
  }

  function bootLabel() {
    if (root.boot.disabled) return "Disabled until you re-enable it."
    if (root.boot.skipOnce) return "Will skip the next boot only."
    return "Runs on boot."
  }

  function lastBootLink() {
    if (!root.lastBoot) return ""
    var when = String(root.lastBoot.startedAt || "").replace("T", " ")
    var n = (root.lastBoot.launches || []).length
    var extra = ""
    if (root.lastBoot.outcome === "launched") extra = " · " + n + " app" + (n === 1 ? "" : "s")
    else if (root.lastBoot.outcome) extra = " · " + root.lastBoot.outcome
    return "Last boot log" + extra + (when ? " · " + when : "")
  }

  Process {
    id: actionProc
    command: [root.enginePath, "list", "--json"]
    stdout: StdioCollector {
      onStreamFinished: { root.applyResult(this.text); root.busy = false }
    }
    onExited: function(exitCode) {
      if (exitCode !== 0)
        root.statusText = "Command failed"
      else if (root.statusText === "" && root.pendingStatus !== "")
        root.statusText = root.pendingStatus
      root.pendingStatus = ""
      root.busy = false
    }
  }

  function openLog() {
    var payload = "{}"
    if (root.lastBoot && root.lastBoot.logPath)
      payload = JSON.stringify({ logPath: root.lastBoot.logPath })
    if (root.bar)
      root.bar.run("omarchy-shell shell summon io.github.laytonf.relaunch " + Util.shellQuote(payload))
    else
      Quickshell.execDetached(["omarchy-shell", "shell", "summon", "io.github.laytonf.relaunch", payload])
  }

  component Chip: Rectangle {
    id: chip
    property string label: ""
    property bool danger: false
    signal clicked
    height: Style.space(22)
    width: chipLabel.implicitWidth + Style.space(12)
    radius: Style.space(4)
    color: chipMouse.containsMouse ? Style.hoverFillFor(root.barForeground, Color.accent) : "transparent"
    border.color: root.barForeground
    border.width: 1
    opacity: root.busy ? 0.45 : 1
    Text {
      id: chipLabel
      anchors.centerIn: parent
      text: chip.label
      color: root.barForeground
      font.family: root.bar ? root.bar.fontFamily : Style.font.family
      font.pixelSize: Style.font.caption
    }
    MouseArea {
      id: chipMouse
      anchors.fill: parent
      hoverEnabled: true
      enabled: !root.busy
      onClicked: chip.clicked()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(Math.min(content.implicitHeight, Style.space(520)))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Flickable {
        id: scroller
        anchors.fill: parent
        contentWidth: width
        contentHeight: content.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height

        Column {
          id: content
          width: scroller.width
          spacing: Style.space(10)

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
            text: "Save the layout you have now. Existing Hyprland startup apps stay listed so you can add them, drop one side, or leave them alone."
            color: root.barForeground
            opacity: 0.75
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap
          }

          Rectangle {
            width: parent.width
            height: Style.space(36)
            radius: Style.space(6)
            color: saveMouse.containsMouse ? Style.hoverFillFor(root.barForeground, Color.accent) : "transparent"
            border.color: root.barForeground
            border.width: 1
            opacity: root.busy ? 0.5 : 1.0
            Text {
              anchors.centerIn: parent
              text: root.busy ? "Working…" : "Save Startup App Workspaces"
              color: root.barForeground
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
                root.statusText = ""
                root.run(["save", "--json"])
              }
            }
          }

          Rectangle { width: parent.width; height: 1; color: root.barForeground; opacity: 0.2 }

          Text {
            visible: root.runningRows.length > 0
            width: parent.width
            text: "Running now"
            color: root.barForeground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            font.bold: true
          }

          Repeater {
            model: root.runningRows
            delegate: Column {
              id: runningRow
              required property var modelData
              width: content.width
              spacing: Style.space(4)

              Text {
                width: parent.width
                text: "ws " + modelData.workspace + "  ·  " + modelData.label
                elide: Text.ElideRight
                color: root.barForeground
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
              }

              Chip {
                label: "Add to relaunch"
                onClicked: root.run([
                  "import", "--exec", runningRow.modelData.exec,
                  "--workspace", String(runningRow.modelData.workspace), "--json"
                ])
              }
            }
          }

          Text {
            width: parent.width
            text: root.relaunchRows.length > 0
              ? "Relaunch list (" + root.relaunchRows.length + ")"
              : "No apps on the relaunch list yet"
            color: root.barForeground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            font.bold: true
          }

          Repeater {
            model: root.relaunchRows
            delegate: Column {
              required property var modelData
              width: content.width
              spacing: Style.space(4)

              Row {
                width: parent.width
                spacing: Style.space(8)
                Text {
                  text: "ws " + modelData.workspace
                  width: Style.space(44)
                  color: root.barForeground
                  opacity: modelData.enabled ? 1.0 : 0.4
                  font.family: root.bar ? root.bar.fontFamily : Style.font.family
                  font.pixelSize: Style.font.bodySmall
                }
                Text {
                  text: modelData.label + (modelData.kind === "both" ? "  · also a startup app" : "")
                  width: parent.width - Style.space(52)
                  elide: Text.ElideRight
                  color: root.barForeground
                  opacity: modelData.enabled ? 1.0 : 0.4
                  font.family: root.bar ? root.bar.fontFamily : Style.font.family
                  font.pixelSize: Style.font.bodySmall
                }
              }

              Flow {
                width: parent.width
                spacing: Style.space(6)
                Chip {
                  label: "Remove from relaunch"
                  onClicked: root.run(["drop", "--class", modelData.class, "--json"])
                }
                Chip {
                  visible: modelData.kind === "both" && modelData.startupId
                  label: "Delete startup config"
                  onClicked: root.run(["drop-startup", "--id", modelData.startupId, "--json"])
                }
              }
            }
          }

          Text {
            visible: root.startupRows.length > 0
            width: parent.width
            text: "Startup apps not in relaunch"
            color: root.barForeground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            font.bold: true
          }

          Repeater {
            model: root.startupRows
            delegate: Column {
              id: startupRow
              required property var modelData
              width: content.width
              spacing: Style.space(4)
              property int pickWs: 1

              Text {
                width: parent.width
                text: modelData.label + "  ·  " + modelData.exec
                elide: Text.ElideRight
                color: root.barForeground
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
              }

              Row {
                spacing: Style.space(6)
                Chip {
                  label: "−"
                  onClicked: startupRow.pickWs = Math.max(1, startupRow.pickWs - 1)
                }
                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  text: "ws " + startupRow.pickWs
                  color: root.barForeground
                  font.family: root.bar ? root.bar.fontFamily : Style.font.family
                  font.pixelSize: Style.font.bodySmall
                }
                Chip {
                  label: "+"
                  onClicked: startupRow.pickWs = Math.min(10, startupRow.pickWs + 1)
                }
                Chip {
                  label: "Add to relaunch"
                  onClicked: root.run(["import", "--exec", startupRow.modelData.exec, "--workspace", String(startupRow.pickWs), "--json"])
                }
                Chip {
                  label: "Leave alone"
                  onClicked: root.run(["ignore", "--id", startupRow.modelData.startupId, "--json"])
                }
              }
            }
          }

          Text {
            visible: root.ignoredRows.length > 0
            width: parent.width
            text: "Left alone (still shown here)"
            color: root.barForeground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            font.bold: true
          }

          Repeater {
            model: root.ignoredRows
            delegate: Row {
              required property var modelData
              width: content.width
              spacing: Style.space(8)
              Text {
                width: parent.width - Style.space(90)
                text: modelData.label
                elide: Text.ElideRight
                color: root.barForeground
                opacity: 0.55
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
              }
              Chip {
                label: "Stop ignoring"
                onClicked: root.run(["unignore", "--id", modelData.startupId, "--json"])
              }
            }
          }

          Rectangle { width: parent.width; height: 1; color: root.barForeground; opacity: 0.2 }

          Text {
            width: parent.width
            text: "Relaunch on boot"
            color: root.barForeground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            font.bold: true
          }

          Text {
            width: parent.width
            text: root.bootLabel()
            color: root.barForeground
            opacity: 0.75
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap
          }

          Flow {
            width: parent.width
            spacing: Style.space(6)
            Chip {
              label: "Skip next boot"
              onClicked: root.run(["boot-skip", "--json"], "Skipping the next boot.")
            }
            Chip {
              visible: !root.boot.disabled
              label: "Disable until re-enabled"
              onClicked: root.run(["boot-disable", "--json"], "Disabled on boot.")
            }
            Chip {
              visible: root.boot.disabled || root.boot.skipOnce
              label: "Enable on boot"
              onClicked: root.run(["boot-enable", "--json"], "Enabled on boot.")
            }
          }

          Chip {
            label: root.confirmRemove ? "Click again to remove permanently" : "Remove Relaunch permanently"
            danger: true
            onClicked: {
              if (!root.confirmRemove) {
                root.confirmRemove = true
                root.statusText = "Click again to uninstall Relaunch."
                disarmRemove.restart()
                return
              }
              disarmRemove.stop()
              root.confirmRemove = false
              root.run(["uninstall", "--yes", "--json"], "Relaunch removed.")
            }
          }

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
            visible: root.lastBoot !== null
            text: root.lastBootLink()
            color: root.barForeground
            opacity: 0.7
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }

          Chip {
            label: "View last boot log"
            onClicked: root.openLog()
          }
        }
      }
    }
  }
}
