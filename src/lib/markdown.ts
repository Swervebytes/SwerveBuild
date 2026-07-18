// Zero-dependency, XSS-safe markdown → structured AST.
//
// This parser returns DATA, never HTML. MessageBubble renders the AST with
// plain Svelte markup, so every string flows through Svelte's auto-escaping —
// there is no `{@html}` anywhere in the path. An agent that emits `<script>` or
// `<img onerror=…>` renders as literal text, exactly as before. Links are
// deliberately NOT parsed into anchors here (bare URLs stay plain text) so no
// javascript: href can ever be produced. Fenced code blocks are handled upstream
// in MessageBubble.parseSegments; this only sees the non-code prose segments.

export type Inline =
  | { t: "text"; v: string }
  | { t: "code"; v: string }
  | { t: "strong"; c: Inline[] }
  | { t: "em"; c: Inline[] };

export type Block =
  | { t: "p"; lines: Inline[][] }
  | { t: "h"; level: number; c: Inline[] }
  | { t: "ul"; items: Inline[][] }
  | { t: "ol"; items: Inline[][] }
  | { t: "quote"; lines: Inline[][] }
  | { t: "table"; header: Inline[][]; rows: Inline[][][] };

// One inline token: code span, bold, or italic. Code is matched first so its
// contents are never re-parsed. Non-greedy bodies keep adjacent markers apart.
const INLINE_RE = /(`[^`]+?`|\*\*[^*]+?\*\*|__[^_]+?__|\*[^*\s][^*]*?\*|_[^_\s][^_]*?_)/;

export function parseInline(text: string): Inline[] {
  const out: Inline[] = [];
  let rest = text;
  // Bound the loop defensively; malformed input can never spin forever.
  let guard = 0;
  while (rest.length > 0 && guard++ < 10000) {
    const m = INLINE_RE.exec(rest);
    if (!m || m.index === undefined) {
      out.push({ t: "text", v: rest });
      break;
    }
    if (m.index > 0) out.push({ t: "text", v: rest.slice(0, m.index) });
    const tok = m[0];
    if (tok.startsWith("`")) {
      out.push({ t: "code", v: tok.slice(1, -1) });
    } else if (tok.startsWith("**") || tok.startsWith("__")) {
      out.push({ t: "strong", c: parseInline(tok.slice(2, -2)) });
    } else {
      out.push({ t: "em", c: parseInline(tok.slice(1, -1)) });
    }
    rest = rest.slice(m.index + tok.length);
  }
  return out;
}

function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|")) s = s.slice(0, -1);
  return s.split("|").map((c) => c.trim());
}

// A markdown table separator row, e.g. `|---|:--:|` or `--- | ---`.
function isTableSep(line: string): boolean {
  const s = line.trim();
  if (!s.includes("-")) return false;
  return /^\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?$/.test(s);
}

const HEADING_RE = /^(#{1,6})\s+(.*)$/;
const UL_RE = /^\s*[-*+]\s+(.*)$/;
const OL_RE = /^\s*\d+[.)]\s+(.*)$/;
const QUOTE_RE = /^\s*>\s?(.*)$/;

export function parseBlocks(text: string): Block[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let i = 0;
  let para: string[] = [];

  const flushPara = () => {
    if (para.length) {
      blocks.push({ t: "p", lines: para.map(parseInline) });
      para = [];
    }
  };

  while (i < lines.length) {
    const line = lines[i];

    if (line.trim() === "") {
      flushPara();
      i++;
      continue;
    }

    // Table: a `|`-bearing line immediately followed by a separator row.
    if (line.includes("|") && i + 1 < lines.length && isTableSep(lines[i + 1])) {
      flushPara();
      const header = splitRow(line).map(parseInline);
      i += 2;
      const rows: Inline[][][] = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim() !== "") {
        rows.push(splitRow(lines[i]).map(parseInline));
        i++;
      }
      blocks.push({ t: "table", header, rows });
      continue;
    }

    const heading = HEADING_RE.exec(line);
    if (heading) {
      flushPara();
      blocks.push({ t: "h", level: heading[1].length, c: parseInline(heading[2]) });
      i++;
      continue;
    }

    if (UL_RE.test(line)) {
      flushPara();
      const items: Inline[][] = [];
      while (i < lines.length) {
        const m = UL_RE.exec(lines[i]);
        if (!m) break;
        items.push(parseInline(m[1]));
        i++;
      }
      blocks.push({ t: "ul", items });
      continue;
    }

    if (OL_RE.test(line)) {
      flushPara();
      const items: Inline[][] = [];
      while (i < lines.length) {
        const m = OL_RE.exec(lines[i]);
        if (!m) break;
        items.push(parseInline(m[1]));
        i++;
      }
      blocks.push({ t: "ol", items });
      continue;
    }

    if (QUOTE_RE.test(line)) {
      flushPara();
      const qlines: Inline[][] = [];
      while (i < lines.length) {
        const m = QUOTE_RE.exec(lines[i]);
        if (!m) break;
        qlines.push(parseInline(m[1]));
        i++;
      }
      blocks.push({ t: "quote", lines: qlines });
      continue;
    }

    para.push(line);
    i++;
  }
  flushPara();
  return blocks;
}
