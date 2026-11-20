import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { Icon } from "../../shared/ui/Icon";
import { grouped, type Option } from "./stepChoice";

/** Space between the picker and its list, and between the list and the page's edge. */
const GAP = 8;
/** The list never gets shorter than this (it scrolls inside). */
const MIN_ROOM = 160;
const MAX_HEIGHT = 420;
/** The list's width when there is room for it. */
const MENU_WIDTH = 380;

/** The box of the nearest ancestor that scrolls (the Settings page), else the window. */
function scrollArea(el: HTMLElement): { top: number; bottom: number; left: number; right: number } {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const y = getComputedStyle(p).overflowY;
    if (y === "auto" || y === "scroll") {
      const r = p.getBoundingClientRect();
      return { top: r.top, bottom: r.bottom, left: r.left, right: r.right };
    }
  }
  return { top: 0, bottom: window.innerHeight, left: 0, right: window.innerWidth };
}

/** Scroll `item` into view inside `list` only: `scrollIntoView` also scrolls every ancestor,
 *  which slid the whole Settings page sideways. */
function revealIn(list: HTMLElement, item: HTMLElement) {
  const top = item.offsetTop;
  const bottom = top + item.offsetHeight;
  if (top < list.scrollTop) list.scrollTop = top;
  else if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight;
}

interface Props {
  /** "" = the app's own flags ("Theo app") */
  value: string;
  options: Option[];
  disabled?: boolean;
  title?: string;
  label: string;
  onChange: (value: string | undefined) => void;
}

/** The Quyền picker: like a select, but every mode shows what it does under its name, the way
 *  the extensions' mode menu does, grouped into what the extension offers and what only the CLI
 *  has. Opens over the page (nothing below moves) and fades in. */
export function ModeSelect({ value, options, disabled, title, label, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const [up, setUp] = useState(false);
  const [room, setRoom] = useState<number | undefined>(undefined);
  // the list grows to the left of a picker near the right edge, never past the page
  const [side, setSide] = useState<{ end: boolean; width: number }>({ end: false, width: MENU_WIDTH });
  const [active, setActive] = useState(0);
  const wrap = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const id = useId();
  const current = options.find((o) => o.value === value) ?? options[0];

  const close = () => setOpen(false);
  const pick = (o: Option) => {
    setOpen(false);
    if (o.value !== value) onChange(o.value || undefined);
  };

  // open upward when the list would not fit below inside the page that scrolls (the last steps),
  // and never taller than the room it has, so the dialog's edge does not cut it
  useLayoutEffect(() => {
    if (!open || !wrap.current || !menu.current) return;
    const box = wrap.current.getBoundingClientRect();
    const area = scrollArea(wrap.current);
    const height = menu.current.scrollHeight;
    const below = area.bottom - box.bottom - GAP;
    const above = box.top - area.top - GAP;
    const goUp = below < height && above > below;
    setUp(goUp);
    setRoom(Math.max(MIN_ROOM, goUp ? above : below));
    const toRight = area.right - box.left - GAP;
    const toLeft = box.right - area.left - GAP;
    const end = toRight < MENU_WIDTH && toLeft > toRight;
    setSide({ end, width: Math.max(box.width, Math.min(MENU_WIDTH, end ? toLeft : toRight)) });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!wrap.current?.contains(e.target as Node)) close();
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [open]);

  const show = () => {
    setActive(Math.max(0, options.findIndex((o) => o.value === value)));
    setOpen(true);
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (disabled) return;
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show();
      }
      return;
    }
    const last = options.length - 1;
    if (e.key === "Escape") {
      // only the list closes, not the Settings dialog
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(last, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(0, i - 1));
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      setActive(e.key === "Home" ? 0 : last);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      const o = options[active];
      if (o) pick(o);
    } else if (e.key === "Tab") {
      close();
    }
  };

  useEffect(() => {
    const list = menu.current;
    const item = list?.querySelector<HTMLElement>(`[data-index="${active}"]`);
    if (open && list && item) revealIn(list, item);
  }, [active, open]);

  let index = -1;
  return (
    <div className="mode-select" ref={wrap}>
      <button
        type="button"
        className="input mode-select-btn"
        disabled={disabled}
        title={title}
        aria-label={label}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        aria-activedescendant={open ? `${id}-${active}` : undefined}
        onClick={() => (open ? close() : show())}
        onKeyDown={onKey}
      >
        <span>{current?.label}</span>
      </button>
      {open && (
        <div
          ref={menu}
          id={id}
          role="listbox"
          aria-label={label}
          className={`mode-menu${up ? " mode-menu--up" : ""}${side.end ? " mode-menu--end" : ""}`}
          style={{
            width: side.width,
            ...(room !== undefined ? { maxHeight: Math.min(MAX_HEIGHT, room) } : {}),
          }}
        >
          {grouped(options).map((g) => (
            <div key={g.group ?? ""} role="group" aria-label={g.group}>
              {g.group && <div className="mode-menu-group">{g.group}</div>}
              {g.options.map((o) => {
                index += 1;
                const i = index;
                const selected = o.value === value;
                return (
                  <div
                    key={o.value}
                    id={`${id}-${i}`}
                    data-index={i}
                    role="option"
                    aria-selected={selected}
                    className={i === active ? "mode-item mode-item--active" : "mode-item"}
                    onMouseEnter={() => setActive(i)}
                    // keep the focus on the button so the keys keep working
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => pick(o)}
                  >
                    <span className="mode-item-name">{o.label}</span>
                    {o.about && <span className="mode-item-about">{o.about}</span>}
                    {selected && <Icon name="check" size={13} className="mode-item-check" />}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
