// relaunch — engine for the omarchy-relaunch bar widget.
// Captures the current app->workspace layout and generates a Hyprland snippet
// that relaunches those apps into the right workspaces on boot.
//
//	relaunch save [--json]     capture running windows -> config + snippet
//	relaunch generate          rebuild snippet from config
//	relaunch list [--json]     print the table
//	relaunch reload            hyprctl reload
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"

	"github.com/laytonf/omarchy-relaunch/engine/internal/config"
)

type result struct {
	OK      bool           `json:"ok"`
	Error   string         `json:"error,omitempty"`
	Added   int            `json:"added,omitempty"`
	Updated int            `json:"updated,omitempty"`
	Stagger int            `json:"staggerSeconds"`
	Entries []config.Entry `json:"entries"`
	Snippet string         `json:"snippetPath,omitempty"`
	Config  string         `json:"configPath,omitempty"`
}

func has(args []string, flag string) bool {
	for _, a := range args {
		if a == flag {
			return true
		}
	}
	return false
}

func emitJSON(r result) {
	b, _ := json.MarshalIndent(r, "", "  ")
	fmt.Println(string(b))
}

func fail(jsonOut bool, err error) {
	if jsonOut {
		emitJSON(result{OK: false, Error: err.Error()})
	} else {
		fmt.Fprintln(os.Stderr, "error:", err)
	}
	os.Exit(1)
}

func main() {
	if len(os.Args) < 2 {
		usage()
		os.Exit(1)
	}
	args := os.Args[2:]
	jsonOut := has(args, "--json")

	switch os.Args[1] {
	case "save":
		c, err := config.Load()
		if err != nil {
			fail(jsonOut, err)
		}
		added, updated, err := c.Capture()
		if err != nil {
			fail(jsonOut, err)
		}
		if err := c.Save(); err != nil {
			fail(jsonOut, err)
		}
		if err := c.WriteSnippet(); err != nil {
			fail(jsonOut, err)
		}
		if jsonOut {
			emitJSON(result{
				OK: true, Added: added, Updated: updated,
				Stagger: c.StaggerSeconds, Entries: c.Entries,
				Snippet: config.SnippetPath(), Config: config.DefaultPath(),
			})
		} else {
			fmt.Printf("Captured: %d new, %d updated.\n", added, updated)
			fmt.Printf("Snippet: %s\n", config.SnippetPath())
			fmt.Println("Run `relaunch reload` or reboot to apply.")
		}

	case "generate":
		c, err := config.Load()
		if err != nil {
			fail(jsonOut, err)
		}
		if err := c.WriteSnippet(); err != nil {
			fail(jsonOut, err)
		}
		if jsonOut {
			emitJSON(result{OK: true, Snippet: config.SnippetPath()})
		} else {
			fmt.Printf("Wrote %s\n", config.SnippetPath())
		}

	case "list":
		c, err := config.Load()
		if err != nil {
			fail(jsonOut, err)
		}
		if jsonOut {
			emitJSON(result{
				OK: true, Stagger: c.StaggerSeconds, Entries: c.Entries,
				Snippet: config.SnippetPath(), Config: config.DefaultPath(),
			})
			return
		}
		if len(c.Entries) == 0 {
			fmt.Println("No entries. Run `relaunch save`.")
			return
		}
		fmt.Printf("%-4s %-3s %-24s %s\n", "ws", "on", "class", "exec")
		for _, e := range c.Entries {
			on := " "
			if e.Enabled {
				on = "x"
			}
			ex := e.Exec
			if ex == "" {
				ex = config.GuessExec(e.Class) + " *"
			}
			fmt.Printf("%-4d [%s] %-24s %s\n", e.Workspace, on, e.Class, ex)
		}

	case "reload":
		if err := exec.Command("hyprctl", "reload").Run(); err != nil {
			fail(jsonOut, err)
		}
		if jsonOut {
			emitJSON(result{OK: true})
		} else {
			fmt.Println("Hyprland reloaded.")
		}

	case "-h", "--help", "help":
		usage()
	default:
		usage()
		os.Exit(1)
	}
}

func usage() {
	fmt.Print(`relaunch — engine for omarchy-relaunch

  relaunch save [--json]     capture running layout -> config + snippet
  relaunch generate          rebuild snippet from config
  relaunch list [--json]     print the table
  relaunch reload            hyprctl reload

config:  ~/.config/omarchy-relaunch/config.json
snippet: ~/.config/omarchy-relaunch/relaunch.conf
`)
}
