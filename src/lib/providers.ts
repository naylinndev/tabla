import type { Driver, SslMode } from "./api";

/**
 * Connection presets. Each provider maps onto one of Tabla's native drivers
 * (wire-compatible engines and hosted services) and carries the defaults and
 * plain-language steps needed to connect.
 */
export interface Provider {
  id: string;
  label: string;
  driver: Driver;
  /** Two-letter monogram and brand-ish colors for the tile. */
  mono: string;
  from: string;
  to: string;
  port: number;
  sslMode: SslMode;
  user?: string;
  database?: string;
  hostPlaceholder: string;
  urlExample: string;
  steps: string[];
  note?: string;
  group: "Databases" | "Cloud & hosted";
}

export const PROVIDERS: Provider[] = [
  {
    id: "postgres",
    label: "PostgreSQL",
    driver: "postgres",
    mono: "Pg",
    from: "#5b8def",
    to: "#336791",
    port: 5432,
    sslMode: "prefer",
    user: "postgres",
    database: "postgres",
    hostPlaceholder: "localhost",
    urlExample: "postgres://user:password@localhost:5432/mydb",
    group: "Databases",
    steps: [
      "Make sure the server is running (Homebrew: brew services start postgresql, Docker: docker run -p 5432:5432 postgres).",
      "Enter the host and port (default 5432), plus the user and password.",
      "Database is optional; it defaults to the user name. You can switch schemas after connecting.",
      "Leave SSL on Prefer for local servers. Use Require for anything on the internet.",
    ],
  },
  {
    id: "mysql",
    label: "MySQL",
    driver: "mysql",
    mono: "My",
    from: "#f4a340",
    to: "#d9730d",
    port: 3306,
    sslMode: "prefer",
    user: "root",
    hostPlaceholder: "localhost",
    urlExample: "mysql://root:password@localhost:3306/mydb",
    group: "Databases",
    steps: [
      "Start the server (Homebrew: brew services start mysql, Docker: docker run -p 3306:3306 -e MYSQL_ROOT_PASSWORD=secret mysql).",
      "Enter host, port (default 3306), user and password.",
      "Database is optional. Leave it empty to browse every database from the sidebar.",
    ],
  },
  {
    id: "mariadb",
    label: "MariaDB",
    driver: "mysql",
    mono: "Ma",
    from: "#4f9fd6",
    to: "#003545",
    port: 3306,
    sslMode: "prefer",
    user: "root",
    hostPlaceholder: "localhost",
    urlExample: "mariadb://root:password@localhost:3306/mydb",
    group: "Databases",
    steps: [
      "Start MariaDB (Homebrew: brew services start mariadb).",
      "Enter host, port (default 3306), user and password.",
      "Database is optional. Leave it empty to browse all databases.",
    ],
  },
  {
    id: "sqlite",
    label: "SQLite",
    driver: "sqlite",
    mono: "Lt",
    from: "#5ec8f2",
    to: "#1f86c4",
    port: 0,
    sslMode: "disable",
    hostPlaceholder: "",
    urlExample: "",
    group: "Databases",
    steps: [
      "Click Browse and choose a .db, .sqlite or .sqlite3 file.",
      "Turn on Read-only if you only want to look, so Tabla never writes to the file.",
    ],
  },
  {
    id: "cockroach",
    label: "CockroachDB",
    driver: "postgres",
    mono: "Cr",
    from: "#7a6cff",
    to: "#3a2f9e",
    port: 26257,
    sslMode: "verify-full",
    user: "root",
    database: "defaultdb",
    hostPlaceholder: "your-cluster.cockroachlabs.cloud",
    urlExample: "postgresql://user:password@host:26257/defaultdb?sslmode=verify-full",
    group: "Databases",
    steps: [
      "CockroachDB Cloud: open your cluster and click Connect.",
      "Choose General connection string and copy it.",
      "Paste it into Import from URL. Host, port 26257, user and database fill in automatically.",
      "Local insecure cluster: set SSL to Disable and user to root.",
    ],
  },
  {
    id: "yugabyte",
    label: "YugabyteDB",
    driver: "postgres",
    mono: "Yb",
    from: "#ff7b5c",
    to: "#c4402a",
    port: 5433,
    sslMode: "prefer",
    user: "yugabyte",
    database: "yugabyte",
    hostPlaceholder: "localhost",
    urlExample: "postgres://yugabyte:password@localhost:5433/yugabyte",
    group: "Databases",
    steps: [
      "YugabyteDB speaks PostgreSQL (YSQL) on port 5433.",
      "Enter host, user (default yugabyte) and password.",
      "For Yugabyte Managed, set SSL to Require.",
    ],
  },
  {
    id: "supabase",
    label: "Supabase",
    driver: "postgres",
    mono: "Sb",
    from: "#4ee0a4",
    to: "#1d9e6b",
    port: 5432,
    sslMode: "require",
    user: "postgres",
    database: "postgres",
    hostPlaceholder: "aws-0-region.pooler.supabase.com",
    urlExample: "postgresql://postgres.PROJECT_REF:PASSWORD@aws-0-region.pooler.supabase.com:5432/postgres",
    group: "Cloud & hosted",
    steps: [
      "In the Supabase dashboard, open your project and click Connect at the top.",
      "Under Connection string, choose the Session pooler URI (it works on IPv4 networks).",
      "Copy it, replace [YOUR-PASSWORD] with your database password, and paste it into Import from URL.",
      "Keep SSL on Require.",
    ],
    note: "Forgot the password? Reset it in Project Settings > Database.",
  },
  {
    id: "neon",
    label: "Neon",
    driver: "postgres",
    mono: "Ne",
    from: "#6cf2c2",
    to: "#14a37a",
    port: 5432,
    sslMode: "require",
    database: "neondb",
    hostPlaceholder: "ep-name-123456.region.aws.neon.tech",
    urlExample: "postgresql://user:password@ep-name-123456.region.aws.neon.tech/neondb?sslmode=require",
    group: "Cloud & hosted",
    steps: [
      "In the Neon console, open your project's Dashboard and click Connect.",
      "Pick the branch and database, then copy the connection string.",
      "Paste it into Import from URL. SSL must stay on Require.",
    ],
  },
  {
    id: "timescale",
    label: "Timescale",
    driver: "postgres",
    mono: "Ts",
    from: "#f7c35c",
    to: "#d68b16",
    port: 5432,
    sslMode: "require",
    user: "tsdbadmin",
    database: "tsdb",
    hostPlaceholder: "service.project.tsdb.cloud.timescale.com",
    urlExample: "postgres://tsdbadmin:password@host:port/tsdb?sslmode=require",
    group: "Cloud & hosted",
    steps: [
      "Open your service in Timescale Cloud and copy the service URL from the overview.",
      "Paste it into Import from URL. The port is often not 5432, so double-check it.",
      "Keep SSL on Require.",
    ],
  },
  {
    id: "rds",
    label: "Amazon RDS",
    driver: "postgres",
    mono: "Rd",
    from: "#5a7fff",
    to: "#2b4bc4",
    port: 5432,
    sslMode: "require",
    hostPlaceholder: "mydb.abc123.region.rds.amazonaws.com",
    urlExample: "postgres://admin:password@mydb.abc123.region.rds.amazonaws.com:5432/postgres",
    group: "Cloud & hosted",
    steps: [
      "In the RDS console, open your database and copy the Endpoint and Port from Connectivity & security.",
      "For RDS MySQL or Aurora MySQL, switch the engine to MySQL below.",
      "The instance must be Publicly accessible, or you need to be on its VPC or VPN.",
      "Its security group must allow inbound traffic on the port from your IP.",
    ],
  },
  {
    id: "redshift",
    label: "Redshift",
    driver: "postgres",
    mono: "Rs",
    from: "#8a7dff",
    to: "#4a3fc0",
    port: 5439,
    sslMode: "require",
    user: "awsuser",
    database: "dev",
    hostPlaceholder: "cluster.abc123.region.redshift.amazonaws.com",
    urlExample: "postgres://awsuser:password@cluster.abc123.region.redshift.amazonaws.com:5439/dev",
    group: "Cloud & hosted",
    steps: [
      "In the Redshift console, open the cluster or workgroup and copy its Endpoint (drop the trailing :5439/dev).",
      "Enter the admin user and password. The default port is 5439.",
      "The cluster must be publicly accessible, or reachable over your VPN.",
    ],
    note: "Redshift support is in beta: browsing and queries work, but some structure details may be missing.",
  },
  {
    id: "planetscale",
    label: "PlanetScale",
    driver: "mysql",
    mono: "Ps",
    from: "#9aa3b2",
    to: "#3b414d",
    port: 3306,
    sslMode: "verify-full",
    hostPlaceholder: "aws.connect.psdb.cloud",
    urlExample: "mysql://username:password@aws.connect.psdb.cloud/database",
    group: "Cloud & hosted",
    steps: [
      "In PlanetScale, open your database and click Connect.",
      "Create a password, then choose General (or Other) as the framework.",
      "Copy the host, username, password and database, or paste the mysql:// URL into Import from URL.",
      "SSL must stay on Verify.",
    ],
  },
  {
    id: "tidb",
    label: "TiDB Cloud",
    driver: "mysql",
    mono: "Ti",
    from: "#ff6b81",
    to: "#c2283f",
    port: 4000,
    sslMode: "verify-full",
    hostPlaceholder: "gateway01.region.prod.aws.tidbcloud.com",
    urlExample: "mysql://user.root:password@gateway01.region.prod.aws.tidbcloud.com:4000/test",
    group: "Cloud & hosted",
    steps: [
      "In TiDB Cloud, open your cluster and click Connect.",
      "Choose General as the connection type and generate a password.",
      "Copy host, port (4000), user and password into Tabla. SSL must stay on Verify.",
    ],
  },
];

export const providerById = (id: string | undefined, driver: Driver): Provider =>
  PROVIDERS.find((p) => p.id === id) ?? PROVIDERS.find((p) => p.id === driver)!;

/** Guesses the hosted provider from a host name (used when importing URLs). */
export function detectProvider(host: string, driver: Driver): string {
  const h = host.toLowerCase();
  const rules: [RegExp, string][] = [
    [/supabase\.(co|com)$/, "supabase"],
    [/neon\.tech$/, "neon"],
    [/(tsdb\.cloud\.timescale\.com|timescale\.com)$/, "timescale"],
    [/cockroachlabs\.cloud$/, "cockroach"],
    [/redshift(-serverless)?\.amazonaws\.com$/, "redshift"],
    [/rds\.amazonaws\.com$/, "rds"],
    [/psdb\.cloud$/, "planetscale"],
    [/tidbcloud\.com$/, "tidb"],
    [/yugabyte\.cloud$/, "yugabyte"],
  ];
  return rules.find(([re]) => re.test(h))?.[1] ?? driver;
}
