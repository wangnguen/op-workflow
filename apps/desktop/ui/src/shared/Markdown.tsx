import type { ReactNode } from "react";
import { openLink } from "../ipc/client";

/**
 * The little markdown an AI writes in a plan or a summary, and what OpenProject's editor stores:
 * headings, lists, fenced code, `code`, **bold**, links (`[text](url)`, `<url>`, bare https
 * URLs). Leftover HTML from the editor (`<br>`, `<p>`, `&nbsp;`) is turned into text first.
 * Built as React elements (never raw HTML), so text from the AI cannot inject markup. A line
 * break inside a paragraph stays a line break.
 */
export function Markdown({ text, className }: { text: string; className?: string }) {
  return <div className={className ? `md ${className}` : "md"}>{blocks(cleanHtml(text))}</div>;
}

type Block =
  | { kind: "h"; level: number; text: string }
  | { kind: "p"; text: string }
  | { kind: "ul" | "ol"; items: string[] }
  | { kind: "code"; text: string };

const ENTITIES: Record<string, string> = {
  nbsp: " ",
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  "#39": "'",
};

/** One piece of text outside code: `<br>` and `</p>` become line breaks, inline tags go (bold
 *  and italics stay as markdown), entities are decoded. */
function cleanPiece(text: string): string {
  if (!/[<&]/.test(text)) return text;
  return text
    .replace(/<br\s*\/?>|<\/(p|div)\s*>/gi, "\n")
    .replace(/<\/?(p|div|span|font|strong|b|em|i|u|ins|mark|figure|figcaption)(\s[^<>]*)?>/gi, (_m, tag: string) => {
      const t = tag.toLowerCase();
      return t === "strong" || t === "b" ? "**" : t === "em" || t === "i" ? "_" : "";
    })
    .replace(/&#(x[0-9a-f]{1,6}|[0-9]{1,7});/gi, (m, code: string) => {
      const n = code[0] === "x" || code[0] === "X" ? parseInt(code.slice(1), 16) : parseInt(code, 10);
      if (!Number.isFinite(n) || n <= 0 || n > 0x10ffff) return m;
      return n === 0xa0 ? " " : String.fromCodePoint(n);
    })
    .replace(/&(nbsp|lt|gt|quot|apos|#39);/gi, (_m, name: string) => ENTITIES[name.toLowerCase()] ?? _m)
    .replace(/&amp;/gi, "&")
    .replace(/ /g, " ");
}

/** HTML left in markdown, cleaned outside fenced code and `code spans`. */
export function cleanHtml(text: string): string {
  if (!/[<&]/.test(text)) return text;
  let fence: string | null = null;
  return text
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((line) => {
      const marker = /^\s*(```|~~~)/.exec(line)?.[1];
      if (fence) {
        if (marker === fence) fence = null;
        return line;
      }
      if (marker) {
        fence = marker;
        return line;
      }
      return line
        .split("`")
        .map((piece, i) => (i % 2 === 0 ? cleanPiece(piece) : piece))
        .join("`");
    })
    .join("\n");
}

export function parseBlocks(text: string): Block[] {
  const out: Block[] = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    const fence = /^\s*(```|~~~)/.exec(line);
    if (fence) {
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !(lines[i] ?? "").trimStart().startsWith(fence[1] ?? "```")) {
        body.push(lines[i] ?? "");
        i += 1;
      }
      out.push({ kind: "code", text: body.join("\n") });
      i += 1;
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      out.push({ kind: "h", level: heading[1]?.length ?? 2, text: heading[2] ?? "" });
      i += 1;
      continue;
    }
    const list = /^\s*([-*+]|\d+[.)])\s+/.exec(line);
    if (list) {
      const ordered = /\d/.test(list[1] ?? "");
      const items: string[] = [];
      while (i < lines.length) {
        const m = /^\s*([-*+]|\d+[.)])\s+(.*)$/.exec(lines[i] ?? "");
        // a bullet list and a numbered list next to each other stay two lists
        if (m && /\d/.test(m[1] ?? "") === ordered) {
          items.push(m[2] ?? "");
        } else if (/^\s{2,}\S/.test(lines[i] ?? "") && items.length > 0) {
          items[items.length - 1] += ` ${(lines[i] ?? "").trim()}`;
        } else break;
        i += 1;
      }
      out.push({ kind: ordered ? "ol" : "ul", items });
      continue;
    }
    if (!line.trim()) {
      i += 1;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && (lines[i] ?? "").trim() && !/^(#{1,6}\s|\s*([-*+]|\d+[.)])\s|\s*(```|~~~))/.test(lines[i] ?? "")) {
      para.push((lines[i] ?? "").trim());
      i += 1;
    }
    out.push({ kind: "p", text: para.join("\n") });
  }
  return out;
}

/** A URL's end, without the punctuation of the sentence around it. */
function trimUrl(url: string): string {
  let u = url.replace(/[.,;:!?'"]+$/, "");
  // a ")" closes the URL only when the URL opened one
  while (u.endsWith(")") && (u.match(/\(/g)?.length ?? 0) < (u.match(/\)/g)?.length ?? 0)) u = u.slice(0, -1);
  return u;
}

function Link({ url, children }: { url: string; children: ReactNode }) {
  return (
    <a
      className="md-link"
      href={url}
      title={url}
      onClick={(e) => {
        e.preventDefault();
        void openLink(url).catch(() => undefined);
      }}
    >
      {children}
    </a>
  );
}

/** `code`, **bold** and links inside one line. */
export function inline(text: string): ReactNode[] {
  const parts: ReactNode[] = [];
  const re = /`([^`]+)`|\*\*([^*]+)\*\*|\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)|<(https?:\/\/[^\s<>]+)>|(https?:\/\/[^\s<>()[\]]+[^\s<>[\]]*)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let n = 0;
  while ((m = re.exec(text)) !== null) {
    let raw = m[0];
    if (m[6] !== undefined) {
      // a bare URL: leave the sentence's punctuation outside it
      const url = trimUrl(m[6]);
      raw = url;
      re.lastIndex = m.index + url.length;
    }
    if (m.index > last) parts.push(text.slice(last, m.index));
    if (m[1] !== undefined) parts.push(<code key={n++}>{m[1]}</code>);
    // bold may hold `code` or a link of its own
    else if (m[2] !== undefined) parts.push(<strong key={n++}>{inline(m[2])}</strong>);
    else if (m[3] !== undefined && m[4] !== undefined) parts.push(<Link key={n++} url={m[4]}>{inline(m[3])}</Link>);
    else if (m[5] !== undefined) parts.push(<Link key={n++} url={m[5]}>{m[5]}</Link>);
    else parts.push(<Link key={n++} url={raw}>{raw}</Link>);
    last = m.index + raw.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

/** A paragraph's lines, each break kept. */
function lines(text: string): ReactNode[] {
  return text.split("\n").flatMap((line, i) => (i === 0 ? inline(line) : [<br key={`br${i}`} />, ...inline(line)]));
}

function blocks(text: string): ReactNode[] {
  return parseBlocks(text).map((b, i) => {
    switch (b.kind) {
      case "h":
        return b.level <= 2 ? <h3 key={i}>{inline(b.text)}</h3> : <h4 key={i}>{inline(b.text)}</h4>;
      case "p":
        return <p key={i}>{lines(b.text)}</p>;
      case "code":
        return (
          <pre key={i}>
            <code>{b.text}</code>
          </pre>
        );
      case "ul":
      case "ol": {
        const Tag = b.kind;
        return (
          <Tag key={i}>
            {b.items.map((item, j) => (
              <li key={j}>{inline(item)}</li>
            ))}
          </Tag>
        );
      }
    }
  });
}
