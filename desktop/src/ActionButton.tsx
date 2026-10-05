import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";

export interface ActionButtonOption {
  id: string;
  label: string;
  note?: string;
  disabled?: boolean;
  /** Include this for choices; omit it for an action that runs immediately. */
  selected?: boolean;
  onClick: () => void;
}

export interface ActionButtonProps {
  label: string;
  onClick: () => void;
  options: ActionButtonOption[];
  menuLabel: string;
  ariaLabel?: string;
  title?: string;
  /** Disable both the main action and its menu. */
  disabled?: boolean;
  /** Disable only the main action so a scope can still be chosen. */
  actionDisabled?: boolean;
  busy?: boolean;
  busyLabel?: string;
  primary?: boolean;
}

/** A main action and an upward-opening menu of related actions or scope choices. */
export function ActionButton({
  label, onClick, options, menuLabel, ariaLabel, title, disabled = false,
  actionDisabled = false, busy = false, busyLabel = "Working…", primary = true,
}: ActionButtonProps) {
  const [open, setOpen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const initialFocus = useRef<"selected" | "first" | "last">("selected");
  const menuId = useId();
  const [position, setPosition] = useState<{ left: number; top: number; width: number; maxHeight: number } | null>(null);
  const positioned = position !== null;
  const menuDisabled = disabled || busy;
  const buttonClass = primary ? "primary" : undefined;

  const close = (restoreFocus = false) => {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus({ preventScroll: true });
  };
  const enabledItems = () => [...(menu.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") || [])];
  const focusItem = (button?: HTMLButtonElement) => {
    if (!button) return;
    button.focus({ preventScroll: true });
    const menuElement = menu.current;
    if (!menuElement) return;
    const item = button.getBoundingClientRect();
    const top = menuElement.getBoundingClientRect().top + menuElement.clientTop;
    const bottom = top + menuElement.clientHeight;
    if (item.top < top || item.height > menuElement.clientHeight) {
      menuElement.scrollTop += item.top - top;
    } else if (item.bottom > bottom) {
      menuElement.scrollTop += item.bottom - bottom;
    }
  };

  useLayoutEffect(() => {
    if (!open) { setPosition(null); return; }
    const placeMenu = () => {
      if (!container.current || !menu.current) return;
      const rect = container.current.getBoundingClientRect();
      const margin = 16;
      const gap = 6;
      const width = Math.min(305, Math.max(0, window.innerWidth - margin * 2));
      const availableAbove = Math.max(0, rect.top - margin - gap);
      const availableBelow = Math.max(0, window.innerHeight - rect.bottom - margin - gap);
      const fullHeight = menu.current.scrollHeight + 2;
      const above = fullHeight <= availableAbove || availableAbove >= availableBelow;
      const maxHeight = above ? availableAbove : availableBelow;
      const height = Math.min(fullHeight, maxHeight);
      setPosition({
        left: Math.max(margin, Math.min(rect.left, window.innerWidth - width - margin)),
        top: above ? Math.max(margin, rect.top - gap - height) : rect.bottom + gap,
        width, maxHeight,
      });
    };
    placeMenu();
    window.addEventListener("resize", placeMenu);
    window.addEventListener("scroll", placeMenu, true);
    return () => {
      window.removeEventListener("resize", placeMenu);
      window.removeEventListener("scroll", placeMenu, true);
    };
  }, [open, options.length]);

  useEffect(() => {
    if (!open || !positioned) return;
    const buttons = enabledItems();
    const preferred = initialFocus.current === "last" ? buttons.at(-1)
      : initialFocus.current === "first" ? buttons[0]
      : buttons.find(button => button.getAttribute("aria-checked") === "true") || buttons[0];
    focusItem(preferred);

    const outside = (event: PointerEvent | FocusEvent) => {
      const target = event.target as Node;
      if (!container.current?.contains(target) && !menu.current?.contains(target)) close();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close(true);
    };
    window.addEventListener("pointerdown", outside);
    window.addEventListener("focusin", outside);
    window.addEventListener("keydown", escape, true);
    return () => {
      window.removeEventListener("pointerdown", outside);
      window.removeEventListener("focusin", outside);
      window.removeEventListener("keydown", escape, true);
    };
  }, [open, positioned]);

  useEffect(() => { if (menuDisabled || !options.length) setOpen(false); }, [menuDisabled, options.length]);

  return <div className={`split-action${options.length ? " has-options" : ""}`} ref={container}>
    <button type="button" className={buttonClass} aria-label={ariaLabel} title={title} disabled={menuDisabled || actionDisabled}
      aria-busy={busy || undefined} onClick={onClick}>{busy ? busyLabel : label}</button>
    {options.length > 0 && <button type="button" className={`${buttonClass || ""} action-menu-trigger`} ref={trigger}
      aria-label={menuLabel} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined}
      title={menuLabel} disabled={menuDisabled}
      onClick={() => { initialFocus.current = "selected"; setOpen(value => !value); }}
      onKeyDown={event => {
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        initialFocus.current = event.key === "ArrowUp" ? "last" : "first";
        if (open) {
          const buttons = enabledItems();
          focusItem(event.key === "ArrowUp" ? buttons.at(-1) : buttons[0]);
        } else setOpen(true);
      }}><ChevronDown size={15} aria-hidden="true" /></button>}
    {open && createPortal(<div className="action-menu" id={menuId} ref={menu} role="menu" aria-label={menuLabel}
      style={{ position: "fixed", bottom: "auto", right: "auto", left: position?.left ?? 16,
        top: position?.top ?? 16, width: position?.width, maxHeight: position?.maxHeight,
        overflowY: "auto", visibility: position ? "visible" : "hidden" }}
      onKeyDown={event => {
        if (event.key === "Tab") { close(true); return; }
        if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        const buttons = enabledItems();
        if (!buttons.length) return;
        const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const index = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
          : (current + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
        focusItem(buttons[index]);
      }}>
      {options.map((option, index) => <button type="button" key={option.id} tabIndex={-1}
        role={option.selected === undefined ? "menuitem" : "menuitemradio"}
        aria-label={option.label} aria-checked={option.selected}
        aria-describedby={option.note ? `${menuId}-note-${index}` : undefined} disabled={option.disabled}
        onClick={() => { close(true); option.onClick(); }}>
        <span className="action-menu-check" aria-hidden="true">{option.selected && <Check size={15} />}</span>
        <span><strong>{option.label}</strong>{option.note && <small id={`${menuId}-note-${index}`}>{option.note}</small>}</span>
      </button>)}
    </div>, document.body)}
  </div>;
}
