import type { Provider } from "./providers";

export interface ErrorHelp {
  title: string;
  tips: string[];
}

/**
 * Turns raw driver errors into plain-language next steps. Written for people
 * who did not set the database up themselves (e.g. it came with their app template).
 */
export function explainError(raw: string, provider?: Provider): ErrorHelp | null {
  const m = raw.toLowerCase();
  const hosted = provider && provider.group === "Cloud & hosted";
  const where = provider ? `${provider.label}'s dashboard` : "your provider's dashboard";

  if (/password authentication failed|access denied for user|authentication failed|invalid password/.test(m))
    return {
      title: "Wrong user name or password",
      tips: [
        "Double-check the user and password. They are case-sensitive.",
        hosted
          ? `Copy a fresh connection string from ${where} and paste it into Import from URL.`
          : "If you never set a password, try leaving the field empty.",
        provider?.id === "supabase"
          ? "Supabase pooler users look like postgres.YOUR_PROJECT_REF, not just postgres."
          : "Some hosts require the user in the form user@host or user.project.",
      ],
    };
  if (/connection refused|os error 61|os error 111|actively refused/.test(m))
    return {
      title: "Nothing is listening at that address",
      tips: [
        "The database server is not running, or the host or port is wrong.",
        "Local databases: start the server (Homebrew: brew services list; Docker: docker ps).",
        `Check the port: PostgreSQL uses 5432 and MySQL 3306${provider ? `; ${provider.label} uses ${provider.port}` : ""}.`,
      ],
    };
  if (/failed to lookup address|nodename nor servname|name or service not known|no such host|dns/.test(m))
    return {
      title: "Couldn't find that host name",
      tips: [
        "Check the host for typos. Paste only the host name, without https:// or a path.",
        hosted ? `Copy the host again from ${where}.` : "Check that you are online, or connected to the right VPN.",
      ],
    };
  if (/timed out|timeout|pool timed out/.test(m))
    return {
      title: "The server didn't answer in time",
      tips: [
        "A firewall is probably blocking the connection.",
        "Cloud databases often only allow listed IPs: add your current IP in the provider's network or allowlist settings.",
        "For Amazon RDS, make the instance Publicly accessible and allow your IP in its security group.",
        provider?.id === "supabase" ? "On IPv4-only networks, use the Session pooler host, not db.PROJECT.supabase.co." : "Check that a VPN isn't required.",
      ],
    };
  if (/ssl|tls|certificate|handshake/.test(m))
    return {
      title: "Secure connection (SSL) problem",
      tips: [
        "Hosted databases usually need SSL set to Require.",
        "Local databases usually don't support SSL: set it to Disable.",
        "If Verify fails, the certificate doesn't match the host. Try Require.",
      ],
    };
  if (/database .* does not exist|unknown database/.test(m))
    return {
      title: "That database doesn't exist",
      tips: [
        "Check the database name. For Postgres, try postgres; for Supabase, postgres; for Neon, neondb.",
        "Leave Database empty for MySQL to see every database you can access.",
      ],
    };
  if (/no pg_hba.conf entry|not allowed to connect|host .* is not allowed/.test(m))
    return {
      title: "The server refused your computer's address",
      tips: [
        "The database only accepts connections from certain IPs. Add yours in the provider's network settings.",
        "If it's your own server, allow your IP in pg_hba.conf (Postgres) or create a user for '%' (MySQL).",
      ],
    };
  if (/too many connections|remaining connection slots/.test(m))
    return {
      title: "The database is out of connection slots",
      tips: ["Close other apps that connect to it, or use the provider's pooler (connection pooling) host and port."],
    };
  return null;
}

/** Builds a prompt the user can paste into any AI assistant. */
export function aiFixPrompt(opts: { engine: string; sql: string; error: string; schema?: string }) {
  return [
    `I'm using ${opts.engine}. This query fails:`,
    "",
    "```sql",
    opts.sql.trim(),
    "```",
    "",
    "Error:",
    "```",
    opts.error.trim(),
    "```",
    ...(opts.schema ? ["", "Relevant schema:", "```", opts.schema.trim(), "```"] : []),
    "",
    "Explain the problem in one or two sentences and give me the corrected query.",
  ].join("\n");
}
