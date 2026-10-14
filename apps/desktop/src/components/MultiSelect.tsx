// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useRef, useState } from "react";
import type { DimensionValue } from "../types";

interface Props {
  label: string;
  options: DimensionValue[];
  selected: string[];
  onChange: (keys: string[]) => void;
}

/** Dropdown with search and checkboxes; empty selection means "all". */
export function MultiSelect({ label, options, selected, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);

  const visible = options.filter((o) => o.label.toLowerCase().includes(search.toLowerCase()));
  const toggle = (key: string) =>
    onChange(selected.includes(key) ? selected.filter((k) => k !== key) : [...selected, key]);
  const summary =
    selected.length === 0
      ? "All"
      : selected.length === 1
        ? (options.find((o) => o.key === selected[0])?.label ?? selected[0])
        : `${selected.length} selected`;

  return (
    <div className="multiselect" ref={root}>
      <button className={selected.length ? "control active" : "control"} onClick={() => setOpen((o) => !o)}>
        <span className="muted">{label}:</span> <span className="clamp-inline">{summary}</span> ▾
      </button>
      {open && (
        <div className="popover">
          {options.length > 8 && (
            <input autoFocus placeholder="Search…" value={search} onChange={(e) => setSearch(e.target.value)} />
          )}
          <ul>
            {visible.map((o) => (
              <li key={o.key}>
                <label className="check">
                  <input type="checkbox" checked={selected.includes(o.key)} onChange={() => toggle(o.key)} />
                  <span>{o.label}</span>
                </label>
              </li>
            ))}
            {visible.length === 0 && <li className="muted small">No matches</li>}
          </ul>
          {selected.length > 0 && (
            <button className="ghost small" onClick={() => onChange([])}>
              Clear
            </button>
          )}
        </div>
      )}
    </div>
  );
}
