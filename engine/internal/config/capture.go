package config

import (
	"sort"

	"github.com/laytonf/omarchy-relaunch/engine/internal/hypr"
)

// Capture inventories running windows and merges them into the config,
// preserving user edits (custom Exec, enabled flag) and only refreshing the
// workspace of known apps. One entry per class (first seen, lowest workspace).
func (c *Config) Capture() (added, updated int, err error) {
	clients, err := hypr.Clients()
	if err != nil {
		return 0, 0, err
	}
	sort.Slice(clients, func(i, j int) bool {
		if clients[i].Workspace.ID != clients[j].Workspace.ID {
			return clients[i].Workspace.ID < clients[j].Workspace.ID
		}
		return clients[i].MatchClass() < clients[j].MatchClass()
	})

	byClass := make(map[string]*Entry, len(c.Entries))
	for i := range c.Entries {
		byClass[c.Entries[i].Class] = &c.Entries[i]
	}

	seen := make(map[string]bool)
	for _, cl := range clients {
		class := cl.MatchClass()
		if class == "" || seen[class] || cl.Workspace.ID < 1 {
			continue
		}
		seen[class] = true
		if e, ok := byClass[class]; ok {
			if e.Workspace != cl.Workspace.ID {
				e.Workspace = cl.Workspace.ID
				updated++
			}
			continue
		}
		c.Entries = append(c.Entries, Entry{
			Class:     class,
			Workspace: cl.Workspace.ID,
			Exec:      GuessExec(class),
			Enabled:   true,
			Float:     cl.Floating,
		})
		added++
	}
	return added, updated, nil
}
