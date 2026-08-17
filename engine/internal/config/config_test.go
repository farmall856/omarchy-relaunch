package config

import (
	"strings"
	"testing"
)

func TestGuessExec(t *testing.T) {
	if got := GuessExec("brave-browser"); got != "brave" {
		t.Fatalf("GuessExec(brave-browser)=%q", got)
	}
	if got := GuessExec("Alacritty"); got != "alacritty" {
		t.Fatalf("GuessExec(Alacritty)=%q", got)
	}
	if got := GuessExec("VSCodium"); got != "codium" {
		t.Fatalf("GuessExec(VSCodium)=%q", got)
	}
	if got := GuessExec("herdr"); got != "herdr" {
		t.Fatalf("GuessExec(herdr)=%q", got)
	}
	if got := GuessExec("SomeApp"); got != "someapp" {
		t.Fatalf("unknown fallback=%q", got)
	}
}

func TestSnippet(t *testing.T) {
	c := &Config{
		StaggerSeconds: 1,
		Entries: []Entry{
			{Class: "brave-browser", Workspace: 2, Exec: "brave", Enabled: true},
			{Class: "herdr", Workspace: 1, Exec: "herdr", Enabled: true},
			{Class: "org.wezfurlong.wezterm", Workspace: 6, Exec: "wezterm", Enabled: true, Float: true},
			{Class: "disabled", Workspace: 3, Exec: "nope", Enabled: false},
			{Class: "", Workspace: 4, Exec: "empty", Enabled: true},
			{Class: "zero", Workspace: 0, Exec: "zero", Enabled: true},
		},
	}
	got := c.Snippet()
	wantLines := []string{
		"windowrulev2 = workspace 1 silent, class:^(herdr)$",
		"windowrulev2 = workspace 2 silent, class:^(brave-browser)$",
		`windowrulev2 = workspace 6 silent, class:^(org\.wezfurlong\.wezterm)$`,
		`windowrulev2 = float, class:^(org\.wezfurlong\.wezterm)$`,
		"exec-once = sleep 1 && herdr",
		"exec-once = sleep 1 && brave",
		"exec-once = sleep 1 && wezterm",
	}
	for _, line := range wantLines {
		if !strings.Contains(got, line) {
			t.Fatalf("snippet missing %q\n%s", line, got)
		}
	}
	if strings.Contains(got, "disabled") || strings.Contains(got, "empty") || strings.Contains(got, "zero") {
		t.Fatalf("snippet emitted a filtered entry:\n%s", got)
	}
}
