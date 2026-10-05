import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";

export type ScanScope = "all" | "unscanned";

/** Scope choices select files; only the main button starts an inspection. */
export function ScanButton({ count, scope, disabled, selectionPending, scanning, onScan, onScope }: {
  count: number;
  scope: ScanScope | null;
  disabled: boolean;
  selectionPending: boolean;
  scanning: boolean;
  onScan: () => void;
  onScope: (scope: ScanScope) => void;
}) {
  const [open, setOpen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const close = (restoreFocus = false) => {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus({ preventScroll: true });
  };
  useEffect(() => {
    if (!open) return;
    menu.current?.querySelector<HTMLButtonElement>(`[aria-checked="true"]`)?.focus();
    if (!menu.current?.contains(document.activeElement)) menu.current?.querySelector("button")?.focus();
    const outside = (event: PointerEvent) => {
      if (!container.current?.contains(event.target as Node)) close();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close(true);
    };
    window.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", escape, true);
    return () => {
      window.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", escape, true);
    };
  }, [open]);
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  return <div className="split-scan" ref={container}>
    <button className="primary" aria-label="Scan selected tracks" disabled={disabled || selectionPending || !count}
      title="Read the selected FLAC audio for MQA signals. Saved results for other files are retained."
      onClick={onScan}>{scanning ? "Scanning…" : `Scan (${count.toLocaleString()})`}</button>
    <button className="primary scan-scope-trigger" ref={trigger} aria-label="Choose scan scope"
      aria-haspopup="menu" aria-expanded={open} disabled={disabled}
      title="Choose which tracks to check, then click Scan. You can also change individual checkmarks."
      onClick={() => setOpen(value => !value)}
      onKeyDown={event => { if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); setOpen(true); } }}>
      <ChevronDown size={15} />
    </button>
    {open && <div className="scan-scope-menu" ref={menu} role="menu" aria-label="Scan scope"
      onKeyDown={event => {
        if (!["ArrowDown", "ArrowUp", "Home", "End", "Tab"].includes(event.key)) return;
        if (event.key === "Tab") { close(); return; }
        event.preventDefault();
        const buttons = [...(menu.current?.querySelectorAll<HTMLButtonElement>("button") || [])];
        const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const index = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
          : (current + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
        buttons[index]?.focus();
      }}>
      {([
        ["all", "All releases", "Select every FLAC track, including scanned files"],
        ["unscanned", "Unscanned releases only", "Select only new or changed FLAC tracks"],
      ] as const).map(([value, label, note]) => <button key={value} role="menuitemradio"
        aria-label={label} aria-checked={scope === value} onClick={() => { close(true); onScope(value); }}>
        <span className="scan-scope-check">{scope === value && <Check size={15} />}</span>
        <span><strong>{label}</strong><small>{note}</small></span>
      </button>)}
    </div>}
  </div>;
}
