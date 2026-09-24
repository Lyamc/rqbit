import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

/** Approx line box for text-sm / leading-6. */
const LINE_PX = 24;
const MIN_LINES = 1;
const MAX_LINES = 5;
const PAD_Y = 12; // py-1.5 ≈ 6+6

type Props = {
  id?: string;
  value: string;
  disabled?: boolean;
  placeholder?: string;
  onChange: (value: string) => void;
};

/**
 * Progressive URL/magnet field: looks like a single-line input at first,
 * grows to ~4–5 lines when there are newlines or wrapped content, then scrolls.
 * Optional wrap toggle (default on). Line gutter; click a number to select that line.
 */
export const UrlLinesEditor: React.FC<Props> = ({
  id = "add_urls",
  value,
  disabled,
  placeholder,
  onChange,
}) => {
  const taRef = useRef<HTMLTextAreaElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const [wrap, setWrap] = useState(true);
  const [visualLines, setVisualLines] = useState(MIN_LINES);

  const logicalLines = useMemo(
    () => (value.length === 0 ? [""] : value.split("\n")),
    [value],
  );

  const measure = useCallback(() => {
    const el = taRef.current;
    if (!el) return;
    const saved = el.style.height;
    el.style.height = "0px";
    const scroll = el.scrollHeight;
    el.style.height = saved;
    const fromScroll = Math.max(
      MIN_LINES,
      Math.round((scroll - PAD_Y) / LINE_PX),
    );
    setVisualLines(Math.max(fromScroll, logicalLines.length));
  }, [logicalLines.length]);

  useLayoutEffect(() => {
    measure();
  }, [value, wrap, measure]);

  useEffect(() => {
    const onResize = () => measure();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [measure]);

  const displayLines = Math.min(MAX_LINES, Math.max(MIN_LINES, visualLines));
  const heightPx = displayLines * LINE_PX + PAD_Y;
  const showGutter = visualLines > 1 || value.includes("\n");

  const selectLine = (lineIndex: number) => {
    const el = taRef.current;
    if (!el || disabled) return;
    const parts = value.split("\n");
    if (lineIndex < 0 || lineIndex >= parts.length) return;
    let start = 0;
    for (let i = 0; i < lineIndex; i++) {
      start += parts[i].length + 1;
    }
    const end = start + parts[lineIndex].length;
    el.focus();
    el.setSelectionRange(start, end);
  };

  const syncGutterScroll = () => {
    const ta = taRef.current;
    const g = gutterRef.current;
    if (ta && g) g.scrollTop = ta.scrollTop;
  };

  const gutterWidthCh = Math.max(2, String(logicalLines.length).length) + 1.25;

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <label htmlFor={id} className="text-sm">
          Magnet or torrent URL
        </label>
        <label className="flex items-center gap-1.5 text-xs text-secondary select-none cursor-pointer">
          <input
            type="checkbox"
            className="rounded border-divider"
            checked={wrap}
            disabled={disabled}
            onChange={(e) => setWrap(e.target.checked)}
          />
          Wrap
        </label>
      </div>

      <div
        className={`flex border border-divider rounded bg-transparent overflow-hidden focus-within:border-primary ${
          disabled ? "opacity-60" : ""
        }`}
      >
        {showGutter && (
          <div
            ref={gutterRef}
            className="flex-shrink-0 select-none border-r border-divider bg-surface/40 text-secondary text-right font-mono text-xs overflow-y-hidden"
            style={{ width: `${gutterWidthCh}ch`, height: heightPx }}
            aria-hidden
          >
            <div style={{ paddingTop: PAD_Y / 2 }}>
              {logicalLines.map((_, i) => (
                <button
                  key={i}
                  type="button"
                  tabIndex={-1}
                  disabled={disabled}
                  title={`Select line ${i + 1}`}
                  className="block w-full px-1.5 hover:bg-primary/15 hover:text-text"
                  style={{ height: LINE_PX, lineHeight: `${LINE_PX}px` }}
                  onMouseDown={(e) => {
                    // Prevent textarea blur before selection applies.
                    e.preventDefault();
                    selectLine(i);
                  }}
                >
                  {i + 1}
                </button>
              ))}
            </div>
          </div>
        )}

        <textarea
          ref={taRef}
          id={id}
          spellCheck={false}
          wrap={wrap ? "soft" : "off"}
          className="grow w-full min-w-0 bg-transparent py-1.5 px-2 font-mono text-sm leading-6 focus:outline-none focus:ring-0 resize-none overflow-y-auto"
          style={{
            height: heightPx,
            maxHeight: MAX_LINES * LINE_PX + PAD_Y,
            whiteSpace: wrap ? "pre-wrap" : "pre",
            overflowX: wrap ? "hidden" : "auto",
            wordBreak: wrap ? "break-all" : "normal",
          }}
          placeholder={placeholder ?? "magnet:?xt=urn:btih:…"}
          value={value}
          disabled={disabled}
          rows={displayLines}
          onChange={(e) => onChange(e.target.value)}
          onScroll={syncGutterScroll}
        />
      </div>
    </div>
  );
};
