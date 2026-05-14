import { type ReactNode } from "react";
import { X } from "lucide-react";

export type ContextMenuItem = {
  label: string;
  icon: ReactNode;
  onClick: () => void;
};

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}) {
  return (
    <div
      className="fixed z-[60] min-w-44 rounded-md border border-border bg-bg-elevated p-1 shadow-lg"
      style={{ left: x, top: y }}
      onClick={(e) => e.stopPropagation()}
    >
      {items.map((item, i) => (
        <button
          key={i}
          type="button"
          onClick={() => {
            item.onClick();
            onClose();
          }}
          className="flex w-full items-center gap-2 rounded px-2 py-1 text-xs hover:bg-bg-hover"
        >
          {item.icon}
          {item.label}
        </button>
      ))}
      <div className="my-1 border-t border-border" />
      <button
        type="button"
        onClick={onClose}
        className="flex w-full items-center gap-2 rounded px-2 py-1 text-xs text-text-muted hover:bg-bg-hover"
      >
        <X className="h-3.5 w-3.5" />
        Close menu
      </button>
    </div>
  );
}
