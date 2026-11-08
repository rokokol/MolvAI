// SPDX-License-Identifier: MIT
// The MCP snippets: each harness gets a file it really reads, and every snippet names the same
// stdio command

import { describe, expect, it } from "vitest";

import { MCP_ARGS, MCP_COMMAND, mcpCommandLine, mcpProfiles } from "./mcp";

describe("mcpCommandLine", () => {
  it("is the stdio command with no port and no URL", () => {
    expect(mcpCommandLine()).toBe("molva mcp");
    expect(mcpCommandLine()).not.toContain("http");
  });
});

describe("mcpProfiles", () => {
  const profiles = mcpProfiles();

  it("carries a profile for the harnesses the page names", () => {
    const ids = profiles.map((profile) => profile.id);
    for (const wanted of ["claude-code", "codex", "cursor", "vscode", "shell"]) {
      expect(ids).toContain(wanted);
    }
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("gives every harness its own file, except the command profile", () => {
    const files = profiles.filter((profile) => profile.file !== "").map((p) => p.file);
    expect(new Set(files).size).toBe(files.length);
  });

  it("writes JSON that parses and names the command in it", () => {
    for (const profile of profiles.filter((item) => item.syntax === "json")) {
      const parsed = JSON.parse(profile.config) as Record<string, unknown>;
      const servers = (parsed.mcpServers ?? parsed.servers) as Record<
        string,
        { command: string; args: string[] }
      >;
      expect(servers.molva.command, profile.id).toBe(MCP_COMMAND);
      expect(servers.molva.args, profile.id).toEqual(MCP_ARGS);
    }
  });

  it("writes the Codex table with the command and the arguments", () => {
    const codex = profiles.find((profile) => profile.id === "codex");
    expect(codex?.syntax).toBe("toml");
    expect(codex?.config).toContain('[mcp_servers.molva]');
    expect(codex?.config).toContain('command = "molva"');
    expect(codex?.config).toContain('args = ["mcp"]');
  });

  it("shows the command for a terminal", () => {
    const shell = profiles.find((profile) => profile.id === "shell");
    expect(shell?.config).toBe(mcpCommandLine());
  });
});
