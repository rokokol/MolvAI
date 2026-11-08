// SPDX-License-Identifier: MIT
// The MCP section: a stdio process that the harness starts itself
//
// There is no port, no URL and no autostart. `molva mcp` reads JSON-RPC from standard input
// and writes the answers to standard output, so the only thing a harness needs is the command
// and its arguments. The profiles below are the file and the snippet of each harness

/** The program and the argument that start the server */
export const MCP_COMMAND = "molva";
export const MCP_ARGS = ["mcp"];

/** `molva mcp`, the line the page offers to copy */
export function mcpCommandLine(): string {
  return [MCP_COMMAND, ...MCP_ARGS].join(" ");
}

/** The `mcpServers` shape that Claude Code, Claude Desktop and Cursor read */
const JSON_SERVERS = JSON.stringify(
  { mcpServers: { molva: { command: MCP_COMMAND, args: MCP_ARGS } } },
  null,
  2,
);

/** The shape of the VS Code `mcp.json`: the key is `servers` and the type is explicit */
const JSON_VSCODE = JSON.stringify(
  { servers: { molva: { type: "stdio", command: MCP_COMMAND, args: MCP_ARGS } } },
  null,
  2,
);

const CLAUDE_CODE_ADD = "claude mcp add --transport stdio molva -- molva mcp";
const CODEX_ADD = "codex mcp add molva -- molva mcp";

const TOML_CODEX = `[mcp_servers.molva]
command = "${MCP_COMMAND}"
args = [${MCP_ARGS.map((arg) => `"${arg}"`).join(", ")}]`;

export type McpSyntax = "json" | "toml" | "shell";

export interface McpProfile {
  id: string;
  /** The dictionary key of the harness name */
  labelKey: string;
  /** The file that holds the snippet, empty for a command */
  file: string;
  syntax: McpSyntax;
  config: string;
  /** The shell command that registers the server, when the harness has one */
  command?: string;
}

/** The harnesses whose configuration this page can name, each with its own file */
export function mcpProfiles(): McpProfile[] {
  return [
    {
      id: "claude-code",
      labelKey: "settings.mcp.harness.claudeCode",
      file: ".mcp.json",
      syntax: "json",
      config: JSON_SERVERS,
      command: CLAUDE_CODE_ADD,
    },
    {
      id: "codex",
      labelKey: "settings.mcp.harness.codex",
      file: "~/.codex/config.toml",
      syntax: "toml",
      config: TOML_CODEX,
      command: CODEX_ADD,
    },
    {
      id: "cursor",
      labelKey: "settings.mcp.harness.cursor",
      file: "~/.cursor/mcp.json",
      syntax: "json",
      config: JSON_SERVERS,
    },
    {
      id: "vscode",
      labelKey: "settings.mcp.harness.vscode",
      file: ".vscode/mcp.json",
      syntax: "json",
      config: JSON_VSCODE,
    },
    {
      id: "claude-desktop",
      labelKey: "settings.mcp.harness.claudeDesktop",
      file: "claude_desktop_config.json",
      syntax: "json",
      config: JSON_SERVERS,
    },
    {
      id: "shell",
      labelKey: "settings.mcp.harness.shell",
      file: "",
      syntax: "shell",
      config: mcpCommandLine(),
    },
  ];
}
