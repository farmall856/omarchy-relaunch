// Package hypr talks to Hyprland over its JSON IPC by shelling out to hyprctl.
package hypr

import (
	"encoding/json"
	"fmt"
	"os/exec"
)

// Client is one window from `hyprctl clients -j` (only decoded fields).
type Client struct {
	Class        string `json:"class"`
	InitialClass string `json:"initialClass"`
	Title        string `json:"title"`
	InitialTitle string `json:"initialTitle"`
	PID          int    `json:"pid"`
	Floating     bool   `json:"floating"`
	Workspace    struct {
		ID   int    `json:"id"`
		Name string `json:"name"`
	} `json:"workspace"`
	Monitor int `json:"monitor"`
}

// Clients returns every mapped window Hyprland currently knows about.
func Clients() ([]Client, error) {
	out, err := exec.Command("hyprctl", "clients", "-j").Output()
	if err != nil {
		return nil, fmt.Errorf("hyprctl clients: %w", err)
	}
	var cs []Client
	if err := json.Unmarshal(out, &cs); err != nil {
		return nil, fmt.Errorf("decode clients: %w", err)
	}
	return cs, nil
}

// MatchClass prefers initialClass, which is stable across an app's post-launch
// class mutations (Brave/Electron), so windowrule matches keep working.
func (c Client) MatchClass() string {
	if c.InitialClass != "" {
		return c.InitialClass
	}
	return c.Class
}
