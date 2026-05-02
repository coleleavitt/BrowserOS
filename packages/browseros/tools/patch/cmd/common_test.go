package cmd

import (
	"bytes"
	"strings"
	"testing"

	"github.com/spf13/cobra"
)

func TestCommandProgressWritesHumanUpdatesToStderr(t *testing.T) {
	oldJSONOut := jsonOut
	t.Cleanup(func() {
		jsonOut = oldJSONOut
	})
	jsonOut = false

	var stderr bytes.Buffer
	cmd := &cobra.Command{}
	cmd.SetErr(&stderr)

	progress := commandProgress(cmd)
	if progress == nil {
		t.Fatalf("expected human progress reporter")
	}
	progress.Step("Applying 1 patch operation")

	if !strings.Contains(stderr.String(), "Applying 1 patch operation") {
		t.Fatalf("expected progress on stderr, got %q", stderr.String())
	}
}

func TestCommandProgressDisabledForJSON(t *testing.T) {
	oldJSONOut := jsonOut
	t.Cleanup(func() {
		jsonOut = oldJSONOut
	})
	jsonOut = true

	if progress := commandProgress(&cobra.Command{}); progress != nil {
		t.Fatalf("expected nil progress reporter in JSON mode")
	}
}
