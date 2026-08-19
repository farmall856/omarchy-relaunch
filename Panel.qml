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
  // One Process serves every action, so requests that arrive while it is
  // running have to wait their turn rather than vanish. See run().
  property var requestQueue: []
  // Set when applyResult parsed a real error out of the engine's JSON, so the
  // fallback knows there is already a better message than "Command failed".
  property bool engineReportedError: false
  property bool outputUnparseable: false
  property string stderrText: ""
  // Omarchy's own plugins put it plainly: the exit and stream-finished
  // signals have NO guaranteed order. Deciding the status message in
  // whichever handler happens to run last is how the engine's real error got
  // replaced by a generic one. Both are recorded, and settle() decides once
  // both have arrived.
  property int lastExitCode: -1
  property bool sawOutput: false
  property bool sawStderr: false
  // settle() can run more than once for one request -- a stream that arrives
  // late still gets to improve the message -- but the queue must only advance
  // once, and busy must only be released once.
  property bool settled: false
  // True from the moment settle() hands the queue to Qt.callLater until that
  // callback has started the request. In that window actionProc.running is
  // already false, so `running` alone would let a request through and the
  // pending callback would then write its command onto a live Process: the
  // dropped action again, one tick later.
  property bool startingNext: false
  // Failures get their own bordered box under the Save button; ordinary
  // results stay in the quiet line at the bottom.
  property bool statusIsError: false
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

  // Setting `running = true` on a Process that is ALREADY running does
  // nothing: the command property changes but no process starts. Every
  // request that arrived while one was in flight was therefore dropped --
  // silently, and after pendingStatus had already been replaced, so the panel
  // could show the previous operation's success message for an action that
  // never ran. Reopening the panel calls refresh(), and the launch-command
  // field submits on Enter, so this was reachable by ordinary clicking.
  //
  // Queue instead of refusing: a request either runs now or runs next, and
  // nothing is discarded. FIFO with no coalescing, because deciding which
  // requests are equivalent is how actions get dropped again.
  function run(args, okText) {
    root.confirmRemove = false
    // Not `busy`: refresh() sets that BEFORE calling run(), so testing it
    // would queue the ordinary idle open path behind nothing.
    if (actionProc.running || root.startingNext) {
      root.requestQueue = root.requestQueue.concat([{ args: args, okText: okText || "" }])
      root.busy = true
      return
    }
    root.startRequest(args, okText || "", false)
  }

  function startRequest(args, okText, keepStatus) {
    root.pendingStatus = okText
    if (!keepStatus) root.statusText = ""
    root.engineReportedError = false
    root.outputUnparseable = false
    if (!keepStatus) root.statusIsError = false
    root.stderrText = ""
    root.lastExitCode = -1
    root.sawOutput = false
    root.sawStderr = false
    root.settled = false
    root.startingNext = false
    root.busy = true
    actionProc.command = [root.enginePath].concat(args)
    actionProc.running = true
  }

  // Every signal calls this; it does nothing until all three have reported.
  // stderr counts: a nonzero run with no JSON settled as "Command failed"
  // while the stderr callback was still in flight, and that callback only
  // assigned the text -- it never asked for the decision to be made again.
  // That is the same ordering bug as the stdout one, on the other stream.
  function settle() {
    if (root.lastExitCode === -1 || !root.sawOutput || !root.sawStderr) return
    root.decideStatus()
    if (root.settled) return
    root.settled = true
    root.pendingStatus = ""
    if (root.requestQueue.length > 0) {
      var next = root.requestQueue[0]
      root.requestQueue = root.requestQueue.slice(1)
      // Keep the finished operation's message: a queued refresh should not
      // wipe the result the user is reading. callLater so any straggling
      // callback from the run that just ended lands first; startingNext keeps
      // the gap closed meanwhile.
      root.startingNext = true
      Qt.callLater(function() {
        root.startingNext = false
        root.startRequest(next.args, next.okText, true)
      })
      return
    }
    root.busy = false
  }

  // Idempotent on purpose: a stream that arrives after a forced settle can
  // call this again and upgrade the message.
  function decideStatus() {
    if (root.engineReportedError) {
      // applyResult already put the engine's own message up ("parse
      // overrides: …", "no relaunch entry for class: …"). That names the
      // actual problem; never replace it with a generic one.
      root.statusIsError = true
    } else if (root.lastExitCode !== 0) {
      root.statusText = root.stderrText !== "" ? root.stderrText : "Command failed"
      root.statusIsError = true
    } else if (root.outputUnparseable) {
      root.statusText = "Bad engine output"
      root.statusIsError = true
    } else if (root.statusText === "" && root.pendingStatus !== "") {
      root.statusText = root.pendingStatus
      root.statusIsError = false
    }
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
        root.statusText = r.error || "unknown"
        root.engineReportedError = true
        return
      }
      if (r.uninstalled === true) {
        root.statusText = "Relaunch removed."
        root.statusIsError = false
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
        root.statusIsError = false
        root.statusText = "Saved: " + r.added + " new, " + r.updated + " updated."
        if (r.warnings && r.warnings.length > 0)
          root.statusText += " Warning: " + r.warnings.map(function(w) {
            return w.class + " — " + (w.message || "command not found")
          }).join(" ")
      } else if (root.statusText === "" && r.warnings && r.warnings.length > 0) {
        root.statusIsError = false
        root.statusText = r.warnings.map(function(w) {
          return w.class + " — " + (w.message || "command not found")
        }).join(" ")
      }
    } catch (e) {
      // Do not caption it here: an unparseable body on a FAILED run means the
      // real message is on stderr, and settle() is the only place that knows
      // the exit code.
      root.outputUnparseable = true
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
    // waitForEnd, because without it the collector can report a stream that
    // has not been fully read: on a failing run the engine's JSON error was
    // still in flight, applyResult saw an empty body, and the panel fell all
    // the way through to "Command failed".
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        root.applyResult(String(text || ""))
        root.sawOutput = true
        root.settle()
      }
    }
    // Failures that produce no JSON at all -- a die before --json is parsed,
    // a missing engine -- say why on stderr. Without this the panel had
    // nothing to show but "Command failed".
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        // First line, and bounded: this ends up in the ERROR box, and a stuck
        // engine should not be able to grow it without limit.
        var raw = String(text || "").trim()
        var nl = raw.indexOf("\n")
        if (nl >= 0) raw = raw.slice(0, nl)
        root.stderrText = raw.length > 500 ? raw.slice(0, 500) + "…" : raw
        root.sawStderr = true
        root.settle()
      }
    }
    onExited: function(exitCode) {
      root.lastExitCode = exitCode
      root.settle()
      // If a stream never reports -- a process that dies without its
      // collectors finishing -- the panel would stay busy with every chip
      // dead until it is restarted. One event-loop turn after the exit,
      // settle with what arrived. Not a timeout, and not a retry: settle() is
      // re-runnable, so a stream that does arrive afterwards still gets to
      // improve the message.
      Qt.callLater(function() {
        if (root.settled || root.lastExitCode === -1) return
        root.sawOutput = true
        root.sawStderr = true
        root.settle()
      })
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
    property color tint: root.barForeground
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
      color: legendChip.tint
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
            height: Math.max(titleRow.implicitHeight, helpChip.height)

            Row {
              id: titleRow
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(8)

              Text {
                anchors.verticalCenter: parent.verticalCenter
                // Same glyph as the bar button (BarWidget.qml), so the popup
                // names the button that opened it. Keep the two in step.
                // Identity, not a control: plain Text, no chip border and no
                // hover state.
                text: "\uf1da"
                color: root.barForeground
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.subtitle
              }

              Text {
                id: titleText
                anchors.verticalCenter: parent.verticalCenter
                text: "relaunch"
                color: root.barForeground
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.subtitle
                font.bold: true
              }
            }

            IconChip {
              id: helpChip
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              // Plain question mark, not the solid circled variant: the
              // chip already draws the surrounding border.
              glyph: "\uf128"  // nf-fa-question
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
                root.statusIsError = false
                root.run(["save", "--json"])
              }
            }
          }

          // Failures used to land in the quiet grey line at the very bottom of
          // the panel, below everything else, where they were easy to miss.
          // They get their own box here, under the button that most often
          // causes them.
          Item {
            width: parent.width
            visible: root.statusIsError && root.statusText.length > 0
            height: visible ? errBox.height + errLegend.height / 2 : 0

            Rectangle {
              id: errBox
              y: errLegend.height / 2
              width: parent.width
              height: errBody.implicitHeight + Style.space(18)
              radius: Style.space(6)
              color: "transparent"
              border.color: Color.urgent
              border.width: 1
            }

            BorderLegend {
              id: errLegend
              title: "ERROR"
              tint: Color.urgent
            }

            Text {
              id: errBody
              x: Style.space(10)
              y: errBox.y + Style.space(9)
              width: parent.width - Style.space(20)
              text: root.statusText
              color: Color.urgent
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.bodySmall
              wrapMode: Text.WordWrap
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

                    // The row shows a friendly label now, but the pin matches
                    // on class, so class has to stay reachable somewhere.
                    Text {
                      visible: relaunchRow.expanded
                      width: parent.width
                      text: "class " + relaunchRow.modelData.class + " — what the workspace pin matches"
                      wrapMode: Text.WordWrap
                      color: root.barForeground
                      opacity: 0.6
                      font.family: root.bar ? root.bar.fontFamily : Style.font.family
                      font.pixelSize: Style.font.caption
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
              title: "NOT IN RELAUNCH"
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
          // Boot policy and the last-boot log are one section. No divider
          // above it: two bordered boxes already read as separate.
          Item {
            id: bootGroup
            width: content.width
            height: bootBox.height + bootLegend.height / 2

            Rectangle {
              id: bootBox
              y: bootLegend.height / 2
              width: parent.width
              height: bootBody.implicitHeight + Style.space(18)
              radius: Style.space(6)
              color: "transparent"
              border.color: root.barForeground
              border.width: 1
            }

            BorderLegend {
              id: bootLegend
              title: "RELAUNCH ON BOOT"
            }

            Column {
              id: bootBody
              x: Style.space(10)
              y: bootBox.y + Style.space(9)
              width: parent.width - Style.space(20)
              spacing: Style.space(6)

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

          Text {
            width: parent.width
            visible: root.statusText.length > 0 && !root.statusIsError
            text: root.statusText
            color: root.barForeground
            opacity: 0.75
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap
          }

          // Uninstall stands alone at the bottom, centred and in no section.
          Item {
            width: parent.width
            height: removeChip.height
            Chip {
              id: removeChip
              anchors.horizontalCenter: parent.horizontalCenter
              label: root.confirmRemove ? "Click again to remove permanently" : "Remove Relaunch permanently"
              danger: true
              onClicked: {
                if (!root.confirmRemove) {
                  root.confirmRemove = true
                  root.statusIsError = false
                  root.statusText = "Click again to uninstall Relaunch."
                  disarmRemove.restart()
                  return
                }
                disarmRemove.stop()
                root.confirmRemove = false
                root.run(["uninstall", "--yes", "--json"], "Relaunch removed.")
              }
            }
          }
        }
      }
    }
  }
}
