import { useEffect, useState } from "react";
import { Bot, Check, Copy, Info, ShieldCheck, Sparkles } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp } from "../lib/store";
import { DriverIcon } from "./icons";
import { Modal, Switch } from "./ui";

type Tool = { id: string; label: string; where: string; steps: string[]; snippet: (bin: string) => string; lang: string };

const json = (bin: string, key = "mcpServers", extra: Record<string, string> = {}) =>
  JSON.stringify({ [key]: { tabla: { ...extra, command: bin, args: ["mcp"] } } }, null, 2);

const TOOLS: Tool[] = [
  {
    id: "claude-code",
    label: "Claude Code",
    where: "Terminal",
    lang: "bash",
    steps: ["Run this once in your terminal.", "Restart Claude Code, then ask it about your database."],
    snippet: (bin) => `claude mcp add tabla -- "${bin}" mcp`,
  },
  {
    id: "cursor",
    label: "Cursor",
    where: "~/.cursor/mcp.json",
    lang: "json",
    steps: [
      "Open Cursor Settings > MCP > Add new global MCP server (or edit ~/.cursor/mcp.json).",
      "Paste this, save, and check that “tabla” shows a green dot.",
    ],
    snippet: (bin) => json(bin),
  },
  {
    id: "claude-desktop",
    label: "Claude Desktop",
    where: "claude_desktop_config.json",
    lang: "json",
    steps: [
      "Open Claude > Settings > Developer > Edit Config.",
      "Merge this into the file, save, and restart Claude.",
    ],
    snippet: (bin) => json(bin),
  },
  {
    id: "windsurf",
    label: "Windsurf",
    where: "~/.codeium/windsurf/mcp_config.json",
    lang: "json",
    steps: ["Open Windsurf Settings > Cascade > MCP servers > View raw config.", "Paste this and press Refresh."],
    snippet: (bin) => json(bin),
  },
  {
    id: "vscode",
    label: "VS Code",
    where: ".vscode/mcp.json",
    lang: "json",
    steps: ["Create .vscode/mcp.json in your project (Copilot agent mode).", "Paste this, then click Start above the server."],
    snippet: (bin) => json(bin, "servers", { type: "stdio" }),
  },
];

const PROMPTS = [
  "Look at my database schema and explain how the tables relate.",
  "How many users signed up each week for the last 2 months?",
  "Write the SQL for a new “orders” table that fits my existing schema (don't run it).",
  "Find rows in users with a missing or invalid email.",
];

export function AiConnect() {
  const open = useApp((s) => s.aiOpen);
  const setOpen = useApp((s) => s.setAiOpen);
  const connections = useApp((s) => s.connections);
  const save = useApp((s) => s.saveConnection);
  const toast = useApp((s) => s.toast);
  const [bin, setBin] = useState("/Applications/Tabla.app/Contents/MacOS/tabla");
  const [installed, setInstalled] = useState(true);
  const [tool, setTool] = useState(TOOLS[0].id);
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    if (open)
      api
        .mcpCommand()
        .then((r) => {
          setBin(r.binary);
          setInstalled(r.installed);
        })
        .catch(() => {});
  }, [open]);
  if (!open) return null;

  const t = TOOLS.find((x) => x.id === tool)!;
  const shared = connections.filter((c) => c.aiAccess).length;
  const copy = async (text: string, key: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(key);
    setTimeout(() => setCopied(null), 1500);
  };

  return (
    <Modal onClose={() => setOpen(false)} className="modal wide">
      <div className="modal-head">
        <div className="ai-badge">
          <Bot />
        </div>
        <div>
          <h2>Connect your AI</h2>
          <p>Let Claude, Cursor and other AI tools look at your databases through Tabla, read-only.</p>
        </div>
      </div>
      <div className="modal-body">
        <section className="ai-step">
          <div className="ai-step-num">1</div>
          <div className="ai-step-body">
            <b>Choose what your AI can see</b>
            {connections.length === 0 ? (
              <p className="dim">Add a connection first. It will show up here.</p>
            ) : (
              <div className="ai-conns">
                {connections.map((c) => (
                  <div key={c.id} className="ai-conn">
                    <DriverIcon driver={c.driver} provider={c.provider} size={22} />
                    <span className="name">{c.name}</span>
                    {c.tag === "production" && <span className="badge production">prod</span>}
                    <Switch
                      on={c.aiAccess}
                      label={`Share ${c.name} with AI`}
                      onChange={async (v) => {
                        try {
                          await save({ ...c, aiAccess: v, password: undefined });
                        } catch (e) {
                          toast(errorMessage(e), "error");
                        }
                      }}
                    />
                  </div>
                ))}
              </div>
            )}
          </div>
        </section>

        <section className="ai-step">
          <div className="ai-step-num">2</div>
          <div className="ai-step-body">
            <b>Add Tabla to your AI tool</b>
            <div className="segmented ai-tools">
              {TOOLS.map((x) => (
                <button key={x.id} className={x.id === tool ? "on" : ""} onClick={() => setTool(x.id)}>
                  {x.label}
                </button>
              ))}
            </div>
            <ol className="ai-steps">
              {t.steps.map((s, i) => (
                <li key={i}>{s}</li>
              ))}
            </ol>
            <div className="code-block">
              <div className="code-head">
                <span>{t.where}</span>
                <button className="btn ghost" onClick={() => copy(t.snippet(bin), "snippet")}>
                  {copied === "snippet" ? <Check /> : <Copy />} {copied === "snippet" ? "Copied" : "Copy"}
                </button>
              </div>
              <pre>{t.snippet(bin)}</pre>
            </div>
            {!installed && (
              <div className="hint">
                <Info /> This path is where Tabla lives once it's installed. You're running a development build, so
                install Tabla before using this.
              </div>
            )}
          </div>
        </section>

        <section className="ai-step">
          <div className="ai-step-num">3</div>
          <div className="ai-step-body">
            <b>Ask away</b>
            <div className="ai-prompts">
              {PROMPTS.map((p) => (
                <button key={p} className="ai-prompt" onClick={() => copy(p, p)} title="Copy prompt">
                  <Sparkles /> <span>{p}</span> {copied === p ? <Check className="ok" /> : <Copy className="dim" />}
                </button>
              ))}
            </div>
          </div>
        </section>

        <div className="toggle-row">
          <ShieldCheck />
          <div>
            <b>Read-only by design</b>
            <small>
              AI tools only see connections you switch on above. Every query runs as one read-only statement in a
              transaction that is always rolled back. For a hard guarantee, connect with a database user that only has
              SELECT rights.
            </small>
          </div>
        </div>
      </div>
      <div className="modal-foot">
        <span className="dim" style={{ fontSize: 12 }}>
          {shared} of {connections.length} connection{connections.length === 1 ? "" : "s"} shared
        </span>
        <div className="spacer" />
        <button className="btn primary" onClick={() => setOpen(false)}>
          Done
        </button>
      </div>
    </Modal>
  );
}
