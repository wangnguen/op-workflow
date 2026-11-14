import type { ReactNode } from "react";

/**
 * The little markdown an AI writes in a plan or a summary: headings, lists, fenced code,
 * `code` and **bold**. Built as React elements (never raw HTML), so text from the AI cannot
 * inject markup.
 */
export function Markdown({ text, className }: { text: string; className?: string }) {
  return <div className={className ? `md ${className}` : "md"}>{blocks(text)}</div>;
}

type Block =
  | { kind: "h"; level: number; text: string }
  | { kind: "p"; text: string }
  | { kind: "ul" | "ol"; items: string[] }
  | { kind: "code"; text: string };

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
    out.push({ kind: "p", text: para.join(" ") });
  }
  return out;
}

/** `code` and **bold** inside one line. */
export function inline(text: string): ReactNode[] {
  const parts: ReactNode[] = [];
  const re = /`([^`]+)`|\*\*([^*]+)\*\*/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let n = 0;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) parts.push(text.slice(last, m.index));
    if (m[1] !== undefined) parts.push(<code key={n++}>{m[1]}</code>);
    // bold may hold `code` of its own
    else parts.push(<strong key={n++}>{inline(m[2] ?? "")}</strong>);
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

function blocks(text: string): ReactNode[] {
  return parseBlocks(text).map((b, i) => {
    switch (b.kind) {
      case "h":
        return b.level <= 2 ? <h3 key={i}>{inline(b.text)}</h3> : <h4 key={i}>{inline(b.text)}</h4>;
      case "p":
        return <p key={i}>{inline(b.text)}</p>;
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
