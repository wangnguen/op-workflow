import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { findSessionFiles, getSessionCommands } from "../../ipc/client";
import type { ImageIn, SlashCommand } from "../../ipc/types";

const t = vi.workspace.composer;
const IMAGE_KINDS = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const IMAGE_MAX_BYTES = 5 * 1024 * 1024;
const IMAGES_MAX = 8;

// ---------- pasted images ----------

function readImage(file: File): Promise<ImageIn> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result ?? "");
      resolve({ mediaType: file.type, data: url.slice(url.indexOf(",") + 1) });
    };
    reader.onerror = () => reject(reader.error ?? new Error(t.imageUnreadable));
    reader.readAsDataURL(file);
  });
}

/** Images pasted or dropped into the input, sent with the next message. */
export function useAttachments(onError: (message: string) => void) {
  const [images, setImages] = useState<ImageIn[]>([]);
  const add = async (files: File[]) => {
    const picked = files.filter((f) => f.type.startsWith("image/"));
    if (picked.length === 0) return false;
    for (const f of picked) {
      if (!IMAGE_KINDS.includes(f.type)) return onError(t.imageKind), true;
      if (f.size > IMAGE_MAX_BYTES) return onError(t.imageSize), true;
    }
    const read = await Promise.all(picked.map(readImage)).catch(() => undefined);
    if (!read) return onError(t.imageUnreadable), true;
    setImages((now) => {
      const next = [...now, ...read];
      if (next.length > IMAGES_MAX) onError(t.imageCount(IMAGES_MAX));
      return next.slice(0, IMAGES_MAX);
    });
    return true;
  };
  return {
    images,
    clear: () => setImages([]),
    remove: (i: number) => setImages((now) => now.filter((_, k) => k !== i)),
    /** a paste that holds images adds them and does not paste text */
    onPaste: (e: React.ClipboardEvent) => {
      const files = [...e.clipboardData.files];
      if (files.some((f) => f.type.startsWith("image/"))) {
        e.preventDefault();
        void add(files);
      }
    },
    onDrop: (e: React.DragEvent) => {
      const files = [...e.dataTransfer.files];
      if (files.some((f) => f.type.startsWith("image/"))) {
        e.preventDefault();
        void add(files);
      }
    },
  };
}

export function AttachmentStrip({ images, onRemove }: { images: ImageIn[]; onRemove: (i: number) => void }) {
  if (images.length === 0) return null;
  return (
    <div className="ws-attach" aria-label={t.images}>
      {images.map((img, i) => (
        <span key={i} className="ws-attach-item">
          <img src={`data:${img.mediaType};base64,${img.data}`} alt={t.imageAlt(i + 1)} />
          <button type="button" className="ws-attach-remove" aria-label={t.imageRemove} title={t.imageRemove}
            onClick={() => onRemove(i)}>
            ×
          </button>
        </span>
      ))}
    </div>
  );
}

// ---------- @ files and / commands ----------

interface Item {
  /** what replaces the token */
  insert: string;
  label: string;
  /** under the label: a file's folder, a command's description */
  about?: string;
  /** a command's origin (repo, user, skill…) */
  tag?: string;
}

interface Token {
  kind: "file" | "command";
  /** where the token (with its @ or /) starts in the text */
  start: number;
  query: string;
}

/** The token being typed before the caret: `@que` anywhere, `/com` at the very start. */
export function tokenAt(text: string, caret: number): Token | undefined {
  const before = text.slice(0, caret);
  const command = /^\/([\w:.-]*)$/.exec(before);
  if (command) return { kind: "command", start: 0, query: command[1] ?? "" };
  const file = /(^|\s)@([^\s@]*)$/.exec(before);
  if (file) return { kind: "file", start: before.length - (file[2]?.length ?? 0) - 1, query: file[2] ?? "" };
  return undefined;
}

const SOURCE: Record<string, string> = t.sources;

/** Suggestions for the token at the caret, like the extension's `@` and `/`. */
export function useSuggest(sessionId: string | undefined, text: string, caret: number) {
  const [items, setItems] = useState<Item[]>([]);
  const [active, setActive] = useState(0);
  const [closed, setClosed] = useState<string>();
  const commands = useRef<{ id?: string; list?: SlashCommand[] }>({});
  const token = sessionId ? tokenAt(text, caret) : undefined;
  const key = token ? `${token.kind}:${token.start}:${token.query}` : undefined;
  const open = token !== undefined && closed !== `${token.kind}:${token.start}` && items.length > 0;

  useEffect(() => {
    if (!token || !sessionId) {
      setItems([]);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(async () => {
      try {
        if (token.kind === "file") {
          const files = await findSessionFiles(sessionId, token.query);
          if (!alive) return;
          setItems(files.map((f) => {
            const at = f.lastIndexOf("/");
            return { insert: `@${f} `, label: f.slice(at + 1), about: at > 0 ? f.slice(0, at + 1) : undefined };
          }));
        } else {
          if (commands.current.id !== sessionId || !commands.current.list) {
            commands.current = { id: sessionId, list: await getSessionCommands(sessionId) };
          }
          if (!alive) return;
          const q = token.query.toLowerCase();
          const list = (commands.current.list ?? [])
            .filter((c) => c.name.toLowerCase().includes(q))
            .sort((a, b) => Number(!a.name.toLowerCase().startsWith(q)) - Number(!b.name.toLowerCase().startsWith(q)));
          setItems(list.slice(0, 40).map((c) => ({
            insert: `/${c.name} `, label: `/${c.name}`, about: c.description, tag: SOURCE[c.source] ?? c.source,
          })));
        }
        setActive(0);
      } catch {
        if (alive) setItems([]);
      }
    }, token.kind === "file" ? 120 : 0);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
    // the token's text decides what to look up
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, sessionId]);

  /** the text with the token replaced, and where the caret goes */
  const apply = (i: number): { text: string; caret: number } | undefined => {
    const item = items[i];
    if (!token || !item) return undefined;
    const end = token.start + 1 + token.query.length;
    const next = text.slice(0, token.start) + item.insert + text.slice(end);
    return { text: next, caret: token.start + item.insert.length };
  };

  return {
    open,
    items,
    active,
    kind: token?.kind,
    apply,
    setActive,
    move: (d: number) => setActive((a) => (items.length ? (a + d + items.length) % items.length : 0)),
    close: () => token && setClosed(`${token.kind}:${token.start}`),
  };
}

export function SuggestList({
  kind, items, active, onPick, onHover,
}: {
  kind: "file" | "command" | undefined;
  items: Item[];
  active: number;
  onPick: (i: number) => void;
  onHover: (i: number) => void;
}) {
  const list = useRef<HTMLUListElement>(null);
  useEffect(() => {
    list.current?.children[active]?.scrollIntoView({ block: "nearest" });
  }, [active]);
  return (
    <div className="ws-suggest" role="presentation">
      <p className="ws-suggest-head faint">{kind === "command" ? t.commandsHead : t.filesHead}</p>
      <ul ref={list} role="listbox" aria-label={kind === "command" ? t.commandsHead : t.filesHead}>
        {items.map((item, i) => (
          <li
            key={item.insert}
            role="option"
            aria-selected={i === active}
            className={i === active ? "is-active" : undefined}
            onMouseDown={(e) => {
              // keep the focus in the input
              e.preventDefault();
              onPick(i);
            }}
            onMouseEnter={() => onHover(i)}
          >
            <span className="ws-suggest-label">
              <span className={kind === "file" ? "mono" : "mono ws-suggest-cmd"}>{item.label}</span>
              {item.tag && <span className="chip">{item.tag}</span>}
            </span>
            {item.about && <span className="ws-suggest-about faint">{item.about}</span>}
          </li>
        ))}
      </ul>
    </div>
  );
}
