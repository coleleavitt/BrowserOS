package cmd

import (
	"fmt"

	"github.com/browseros-ai/BrowserOS/packages/browseros/tools/patch/internal/ui"
	"github.com/spf13/cobra"
)

func init() {
	command := &cobra.Command{
		Use:         "list",
		Aliases:     []string{"ls"},
		Annotations: map[string]string{"group": "Workspace:"},
		Short:       "List registered workspaces",
		Args:        cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(appState.Registry.Workspaces) == 0 {
				return renderResult(map[string]any{"workspaces": []any{}}, func() {
					fmt.Println("No workspaces registered. Run `browseros-patch add <name> <path>`.")
				})
			}
			rows := make([][]string, 0, len(appState.Registry.Workspaces))
			for _, ws := range appState.Registry.Workspaces {
				rows = append(rows, []string{
					ws.Name,
					ws.Path,
				})
			}
			return renderResult(map[string]any{"workspaces": appState.Registry.Workspaces}, func() {
				fmt.Println(ui.RenderTable([]string{"NAME", "PATH"}, rows))
			})
		},
	}
	rootCmd.AddCommand(command)
}
