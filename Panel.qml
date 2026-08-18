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
  property string execEditClass: ""
  property var warnings: []

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

  // Everything not on the relaunch list, in one box: running windows that
  // could be added, startup apps not yet imported, and startup apps the user
  // chose to leave alone. Each row carries its own `kind`, which decides
  // which actions it gets.
  readonly property var notInRelaunchRows: root.runningRows
    .concat(root.startupRows)
    .concat(root.ignoredRows)

  // One bordered box per workspace. relaunchRows is already sorted by
  // workspace, so a run of equal workspace numbers is one group.
  readonly property var relaunchGroups: {
    var groups = []
    var current = null
    for (var i = 0; i < root.relaunchRows.length; i++) {
      var row = root.relaunchRows[i]
      if (current === null || current.workspace !== row.workspace) {
        current = { workspace: row.workspace, apps: [] }
        groups.push(current)
      }
      current.apps.push(row)
    }
    return groups
  }

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
      if (r.warnings !== undefined) root.warnings = r.warnings
      if (r.added !== undefined) {
        root.statusText = "Saved: " + r.added + " new, " + r.updated + " updated."
        if (r.warnings && r.warnings.length > 0)
          root.statusText += " Warning: " + r.warnings.map(function(w) {
            return w.class + " — " + (w.message || "command not found")
          }).join(" ")
      } else if (root.statusText === "" && r.warnings && r.warnings.length > 0) {
        root.statusText = r.warnings.map(function(w) {
          return w.class + " — " + (w.message || "command not found")
        }).join(" ")
      }
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

  readonly property string helpUrl: "https://github.com/farmall856/omarchy-relaunch#readme"

  function openHelp() {
    if (root.bar)
      root.bar.run("xdg-open " + Util.shellQuote(root.helpUrl))
    else
      Quickshell.execDetached(["xdg-open", root.helpUrl])
  }

  function saveExec(className, exec) {
    var cmd = String(exec || "").replace(/^\s+|\s+$/g, "")
    if (!className || !cmd) return
    root.execEditClass = ""
    root.run(["set-exec", "--class", className, "--exec", cmd, "--json"], "Saved launch command.")
  }

  function rowUnverified(row) {
    return row && (row.execSource === "guess" || row.execSource === "cmdline" || row.unverified === true)
  }

  function rowBroken(row) {
    return row && row.execOk === false
  }

  component Chip: Rectangle {
    id: chip
    property string label: ""
    property bool danger: false
    signal clicked
    height: Style.space(22)
    width: chipLabel.implicitWidth + Style.space(12)
    radius: Style.space(4)
    color: chipMouse.containsMouse ? Style.hoverFillFor(chip.danger ? Color.urgent : root.barForeground, chip.danger ? Color.urgent : Color.accent) : "transparent"
    border.color: chip.danger ? Color.urgent : root.barForeground
    border.width: 1
    opacity: root.busy ? 0.45 : 1
    Text {
      id: chipLabel
      anchors.centerIn: parent
      text: chip.label
      color: chip.danger ? Color.urgent : root.barForeground
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

  // Square, glyph-only variant of Chip. An icon-only control needs a name,
  // so `hint` is required reading for anyone adding one.
  component IconChip: Rectangle {
    id: iconChip
    property string glyph: ""
    property string hint: ""
    property bool danger: false
    property bool active: false
    property real glyphSize: Style.font.bodySmall
    signal clicked
    readonly property color tint: iconChip.danger ? Color.urgent : root.barForeground
    height: Style.space(22)
    width: Style.space(26)
    radius: Style.space(4)
    color: iconChipMouse.containsMouse || iconChip.active
      ? Style.hoverFillFor(iconChip.tint, iconChip.danger ? Color.urgent : Color.accent)
      : "transparent"
    border.color: iconChip.tint
    border.width: 1
    opacity: root.busy ? 0.45 : 1
    Text {
      anchors.centerIn: parent
      text: iconChip.glyph
      color: iconChip.tint
      font.family: root.bar ? root.bar.fontFamily : Style.font.family
      font.pixelSize: iconChip.glyphSize
    }
    MouseArea {
      id: iconChipMouse
      anchors.fill: parent
      hoverEnabled: true
      enabled: !root.busy
      onClicked: iconChip.clicked()
      PanelToolTip {
        visible: iconChipMouse.containsMouse && iconChip.hint !== ""
        text: iconChip.hint
      }
    }
  }

  // Title chip that straddles a box's top border, fieldset style. It paints
  // the panel background behind itself to break the border line, so the
  // heading costs no vertical space inside the box.
  component BorderLegend: Rectangle {
    id: legendChip
    property string title: ""
    x: Style.space(10)
    width: legendLabel.implicitWidth + Style.space(8)
    height: legendLabel.implicitHeight
    // Panel exposes barForeground but no background; the shell's own idiom
    // for one is bar.background with a Color fallback (see Ui/PanelSlider).
    color: root.bar ? root.bar.background : Color.background
    Text {
      id: legendLabel
      anchors.centerIn: parent
      text: legendChip.title
      color: root.barForeground
      opacity: 0.75
      font.family: root.bar ? root.bar.fontFamily : Style.font.family
      font.pixelSize: Style.font.caption
      font.bold: true
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
      blocked: root.execEditClass !== ""
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

          // Lowercase to match how the plugin is named everywhere else.
          Item {
            width: parent.width
            height: Math.max(titleText.implicitHeight, helpChip.height)

            Text {
              id: titleText
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
              text: "relaunch"
              color: root.barForeground
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.subtitle
              font.bold: true
            }

            IconChip {
              id: helpChip
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              glyph: "\uf059"  // nf-fa-question_circle
              hint: "Open the Relaunch README on GitHub"
              onClicked: root.openHelp()
            }
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
              text: root.busy ? "Working…" : "Save Current Workspaces"
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
            model: root.relaunchGroups
            delegate: Item {
              id: wsGroup
              required property var modelData
              width: content.width
              // The legend straddles the top border, so half of it sits
              // above the box and has to be paid for here.
              height: wsBox.height + wsLegend.height / 2

              Rectangle {
                id: wsBox
                y: wsLegend.height / 2
                width: parent.width
                height: wsBody.implicitHeight + Style.space(18)
                radius: Style.space(6)
                color: "transparent"
                border.color: root.barForeground
                border.width: 1
              }

              BorderLegend {
                id: wsLegend
                title: "Workspace " + wsGroup.modelData.workspace
              }

              Column {
                id: wsBody
                x: Style.space(10)
                y: wsBox.y + Style.space(9)
                width: parent.width - Style.space(20)
                spacing: Style.space(6)

                Repeater {
                  model: wsGroup.modelData.apps
                  delegate: Column {
                    id: relaunchRow
                    required property var modelData
                    // A broken row opens its editor unprompted: there is
                    // nothing on the collapsed line to fix it with.
                    property bool expanded: root.rowBroken(modelData)
                    width: wsBody.width
                    spacing: Style.space(4)

                    // One app is one line: the name, then the two actions.
                    Item {
                      width: parent.width
                      height: rowActions.height

                      Text {
                        anchors.left: parent.left
                        anchors.right: rowActions.left
                        anchors.rightMargin: Style.space(6)
                        anchors.verticalCenter: parent.verticalCenter
                        text: relaunchRow.modelData.label
                          + (relaunchRow.modelData.kind === "both" ? "  · also a startup app" : "")
                          + (root.rowUnverified(relaunchRow.modelData) ? "  (unverified)" : "")
                        elide: Text.ElideRight
                        color: root.rowBroken(relaunchRow.modelData) ? Color.urgent : root.barForeground
                        opacity: relaunchRow.modelData.enabled ? 1.0 : 0.4
                        font.family: root.bar ? root.bar.fontFamily : Style.font.family
                        font.pixelSize: Style.font.bodySmall
                      }

                      Row {
                        id: rowActions
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: Style.space(6)
                        IconChip {
                          glyph: "\uf135"  // nf-fa-rocket
                          hint: relaunchRow.expanded ? "Hide launch command" : "View or edit launch command"
                          active: relaunchRow.expanded
                          danger: root.rowBroken(relaunchRow.modelData)
                          onClicked: relaunchRow.expanded = !relaunchRow.expanded
                        }
                        IconChip {
                          glyph: "\uf1f8"  // nf-fa-trash
                          hint: "Remove " + relaunchRow.modelData.label + " from relaunch"
                          onClicked: root.run(["drop", "--class", relaunchRow.modelData.class, "--json"])
                        }
                      }
                    }

                    Text {
                      visible: relaunchRow.expanded && root.rowBroken(relaunchRow.modelData)
                      width: parent.width
                      text: "Launch command not found. Type the command that starts this app."
                      wrapMode: Text.WordWrap
                      color: Color.urgent
                      font.family: root.bar ? root.bar.fontFamily : Style.font.family
                      font.pixelSize: Style.font.caption
                    }

                    Row {
                      visible: relaunchRow.expanded
                      width: parent.width
                      spacing: Style.space(6)
                      TextField {
                        id: execField
                        width: parent.width - Style.space(70)
                        text: relaunchRow.modelData.exec || ""
                        placeholderText: "launch command"
                        font.family: Style.font.family
                        font.pixelSize: Style.font.bodySmall
                        foreground: root.barForeground
                        horizontalPadding: Style.spacing.controlGap
                        verticalPadding: Style.spacing.controlPaddingY
                        onActiveFocusChanged: {
                          if (activeFocus) root.execEditClass = relaunchRow.modelData.class
                          else if (root.execEditClass === relaunchRow.modelData.class) root.execEditClass = ""
                        }
                        onAccepted: root.saveExec(relaunchRow.modelData.class, text)
                        Keys.onEscapePressed: {
                          root.execEditClass = ""
                          focus = false
                          relaunchRow.expanded = false
                        }
                      }
                      Chip {
                        label: "Save"
                        onClicked: root.saveExec(relaunchRow.modelData.class, execField.text)
                      }
                    }

                    Flow {
                      visible: relaunchRow.expanded
                        && relaunchRow.modelData.kind === "both"
                        && relaunchRow.modelData.startupId
                      width: parent.width
                      spacing: Style.space(6)
                      Chip {
                        label: "Delete startup config"
                        onClicked: root.run(["drop-startup", "--id", relaunchRow.modelData.startupId, "--json"])
                      }
                    }
                  }
                }
              }
            }
          }

          // Everything not on the relaunch list, in one box: running
          // windows, startup apps, and the ones left alone. One line each.
          Item {
            id: notInGroup
            visible: root.notInRelaunchRows.length > 0
            width: content.width
            height: visible ? notInBox.height + notInLegend.height / 2 : 0

            Rectangle {
              id: notInBox
              y: notInLegend.height / 2
              width: parent.width
              height: notInBody.implicitHeight + Style.space(18)
              radius: Style.space(6)
              color: "transparent"
              border.color: root.barForeground
              border.width: 1
            }

            BorderLegend {
              id: notInLegend
              title: "Not in relaunch"
            }

            Column {
              id: notInBody
              x: Style.space(10)
              y: notInBox.y + Style.space(9)
              width: parent.width - Style.space(20)
              spacing: Style.space(6)

              Repeater {
                model: root.notInRelaunchRows
                delegate: Item {
                  id: notInRow
                  required property var modelData
                  width: notInBody.width
                  height: notInActions.height

                  Text {
                    anchors.left: parent.left
                    anchors.right: notInActions.left
                    anchors.rightMargin: Style.space(6)
                    anchors.verticalCenter: parent.verticalCenter
                    text: notInRow.modelData.kind === "running"
                      ? "ws " + notInRow.modelData.workspace + "  ·  " + notInRow.modelData.label
                      : notInRow.modelData.label
                    elide: Text.ElideRight
                    color: root.barForeground
                    // Left-alone rows are opted out, so they read as quieter.
                    opacity: notInRow.modelData.kind === "ignored" ? 0.55 : 1.0
                    font.family: root.bar ? root.bar.fontFamily : Style.font.family
                    font.pixelSize: Style.font.bodySmall
                  }

                  Row {
                    id: notInActions
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Style.space(6)

                    IconChip {
                      visible: notInRow.modelData.kind === "startup"
                      glyph: "\uf070"  // nf-fa-eye_slash
                      hint: "Leave this startup app alone"
                      onClicked: root.run(["ignore", "--id", notInRow.modelData.startupId, "--json"])
                    }
                    IconChip {
                      visible: notInRow.modelData.kind === "ignored"
                      glyph: "\uf06e"  // nf-fa-eye
                      hint: "Stop leaving this startup app alone"
                      onClicked: root.run(["unignore", "--id", notInRow.modelData.startupId, "--json"])
                    }
                    IconChip {
                      visible: notInRow.modelData.kind !== "ignored"
                      glyph: "+"
                      glyphSize: Style.font.body
                      // A running window is added where it already is. A
                      // startup app has no window to read a workspace from,
                      // so it starts on 1; launching it and saving moves it.
                      hint: notInRow.modelData.kind === "running"
                        ? "Add " + notInRow.modelData.label + " to relaunch on workspace " + notInRow.modelData.workspace
                        : "Add " + notInRow.modelData.label + " to relaunch on workspace 1"
                      onClicked: notInRow.modelData.kind === "running"
                        ? root.run([
                            "import", "--class", notInRow.modelData.class,
                            "--workspace", String(notInRow.modelData.workspace), "--json"
                          ])
                        : root.run([
                            "import", "--exec", notInRow.modelData.exec,
                            "--workspace", "1", "--json"
                          ])
                    }
                  }
                }
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
              visible: !root.boot.disabled && !root.boot.skipOnce
              label: "Skip next boot"
              onClicked: root.run(["boot-skip", "--json"], "Skipping the next boot.")
            }
            Chip {
              visible: root.boot.disabled || root.boot.skipOnce
              label: "Enable on boot"
              onClicked: root.run(["boot-enable", "--json"], "Enabled on boot.")
            }
            Chip {
              visible: !root.boot.disabled
              label: "Disable until re-enabled"
              onClicked: root.run(["boot-disable", "--json"], "Disabled on boot.")
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
