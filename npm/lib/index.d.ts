/** Options shared by every call: what may be fetched and how it is trusted. */
export interface GlobalOptions {
  /** Allow loopback and private addresses. Off by default. */
  allowPrivate?: boolean;
  /** Restrict fetching to these hosts and their subdomains. */
  allowHosts?: string[];
  /**
   * Which roots to verify against. The default checks the public roots first
   * and falls back to a set including the bundled Russian Trusted Root CA.
   */
  trust?: 'public-then-extra' | 'public-only' | 'combined' | 'none';
  /** Extra root certificates, as paths to PEM bundles. */
  caBundles?: string[];
  /** How long to wait for the process, in milliseconds. */
  timeoutMs?: number;
}

/** Options controlling how a page is rendered. */
export interface RenderOptions extends GlobalOptions {
  /** Run the page's scripts. Default true. */
  javascript?: boolean;
  /** Fetch and run same-origin `<script src>`. Default true. */
  externalScripts?: boolean;
  /** How far the page's virtual clock may run, in milliseconds. */
  horizonMs?: number;
  /** JavaScript heap ceiling, in megabytes. */
  memoryMb?: number;
}

export interface ReadOptions extends RenderOptions {
  /** Cap the Markdown at this many characters. */
  maxChars?: number;
  /** Include images. Default true. */
  images?: boolean;
  /** Include link URLs. Default true; false keeps the link text. */
  links?: boolean;
  /** Include the page's console output in the report. */
  console?: boolean;
}

export interface Feed {
  url: string;
  title: string | null;
  kind: 'rss' | 'atom';
}

/** What one page cost. */
export interface Report {
  url: string;
  final_url: string;
  status: number;
  charset: string;
  /** What the response held: html, pdf, json, image and so on. */
  content_kind: string;
  javascript: boolean;
  scripts_inlined: number;
  scripts_run: number;
  timer_callbacks: number;
  subresource_requests: number;
  /** Virtual milliseconds the page's own clock reached. */
  virtual_ms: number;
  fetch_ms: number;
  render_ms: number;
  extract_ms: number;
  total_ms: number;
  errors?: string[];
  console?: string[];
  /** A navigation the page asked for and we did not follow. */
  requested_navigation?: string;
  /** True when a limit stopped the page rather than quiescence. */
  truncated: boolean;
  /** Set when the response looks like a refusal rather than a page. */
  blocked?: string;
}

export interface Reading {
  title: string | null;
  description: string | null;
  author: string | null;
  site_name: string | null;
  published: string | null;
  modified: string | null;
  canonical_url: string | null;
  image: string | null;
  language: string | null;
  robots: string | null;
  feeds: Feed[];
  schema_types: string[];
  /** The article, as Markdown. */
  content: string;
  /** Characters of extracted text. */
  length: number;
  /** True when no candidate scored well, so this may include navigation. */
  low_confidence: boolean;
  report: Report;
}

export interface BundledCertificate {
  name: string;
  subject: string;
  not_after: string;
  source: string;
}

export declare class MarError extends Error {
  stderr?: string;
  exitCode?: number;
}

/** Read one page: run its scripts, find the article, return Markdown. */
export declare function read(url: string, options?: ReadOptions): Promise<Reading>;

/** Fetch a page and return the HTML after its scripts have run. */
export declare function html(url: string, options?: RenderOptions): Promise<string>;

/** Render a page, then evaluate an expression in it. */
export declare function evaluate(
  url: string,
  expression: string,
  options?: RenderOptions,
): Promise<unknown>;

/** The root certificates bundled into the binary. */
export declare function certificates(
  options?: GlobalOptions,
): Promise<BundledCertificate[]>;

/** The binary's version string, or null if it could not be run. */
export declare function version(): string | null;

/** Where the `mar` binary was found. */
export declare function binaryPath(): string;

export interface ReaderOptions extends GlobalOptions {
  host?: string;
  port?: number;
  workers?: number;
  token?: string;
  startTimeoutMs?: number;
  /** Pass the server's log through to this process's stderr. */
  verbose?: boolean;
}

/** A long-running reader, worth starting above a handful of pages. */
export interface Reader {
  readonly url: string;
  read(url: string, options?: ReadOptions): Promise<Reading>;
  html(url: string, options?: RenderOptions): Promise<{ html: string; report: Report }>;
  evaluate(url: string, expression: string, options?: RenderOptions): Promise<unknown>;
  close(): Promise<void>;
}

export declare function createReader(options?: ReaderOptions): Promise<Reader>;

export interface LaunchOptions extends RenderOptions {
  host?: string;
  port?: number;
  token?: string;
  maxConnections?: number;
  startTimeoutMs?: number;
  verbose?: boolean;
}

/** A running CDP endpoint, for Puppeteer or Playwright to connect to. */
export interface Launched {
  /** Pass this to `puppeteer.connect({ browserWSEndpoint })`. */
  readonly wsEndpoint: string;
  readonly httpEndpoint: string;
  readonly process: import('node:child_process').ChildProcess;
  version(): Promise<Record<string, string>>;
  close(): Promise<void>;
}

export declare function launch(options?: LaunchOptions): Promise<Launched>;

declare const _default: {
  read: typeof read;
  html: typeof html;
  evaluate: typeof evaluate;
  launch: typeof launch;
  createReader: typeof createReader;
  certificates: typeof certificates;
  version: typeof version;
  binaryPath: typeof binaryPath;
};
export default _default;
