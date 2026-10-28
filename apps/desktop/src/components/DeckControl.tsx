// A control drawn the way the deck draws it. Colors come from the deck theme
// variables on an ancestor (`--button`, `--button-fg`, `--accent`, `--radius`),
// sizes scale with the cell through container query units.
import type { Control, Icon, LabelPosition } from "@omni/protocol";
import { cx } from "../lib/util";
import { ControlIcon } from "./Icon";

export function DeckControlView({
  control,
  active = false,
  truncateLabel = false,
}: {
  control: Control;
  active?: boolean;
  /** Keeps labels on one line in the small inspector preview. */
  truncateLabel?: boolean;
}) {
  const { style, kind } = control;
  const bg = (active && style.activeBg) || style.bg || "var(--button)";
  const fg = style.fg ?? "var(--button-fg)";
  const label = active && style.activeLabel ? style.activeLabel : style.label;
  const icon = (active && style.activeIcon) || style.icon;
  return (
    <div
      className="flex size-full overflow-hidden [container-type:size]"
      style={{ background: bg, color: fg, borderRadius: "var(--radius)" }}
    >
      {kind.type === "slider" ? (
        <SliderBody vertical={kind.vertical} icon={icon} label={style.labelPosition === "hidden" ? "" : label} />
      ) : (
        <ButtonBody icon={icon} label={label} position={style.labelPosition} truncateLabel={truncateLabel} />
      )}
    </div>
  );
}

function ButtonBody({ icon, label, position, truncateLabel }: {
  icon: Icon | null;
  label: string;
  position: LabelPosition;
  truncateLabel: boolean;
}) {
  const labelClass = cx(
    "max-w-full px-[6cqmin] text-center leading-tight font-medium",
    truncateLabel ? "truncate" : "line-clamp-2 break-words",
  );
  const showLabel = position !== "hidden" && label !== "";
  if (!icon) {
    return (
      <div className="flex size-full items-center justify-center">
        {showLabel && <span className={labelClass} style={{ fontSize: "max(11px, 15cqmin)" }}>{label}</span>}
      </div>
    );
  }
  const text = showLabel && (
    <span className={labelClass} style={{ fontSize: "max(10px, 12cqmin)" }}>
      {label}
    </span>
  );
  return (
    <div
      className={cx(
        "flex size-full flex-col items-center gap-[4cqmin] py-[8cqmin]",
        position === "center" || !showLabel ? "justify-center" : "justify-between",
      )}
    >
      {position === "top" && text}
      <div className={cx("flex items-center justify-center", position !== "center" && showLabel && "min-h-0 flex-1")}>
        <ControlIcon icon={icon} size={showLabel ? "38cqmin" : "48cqmin"} />
      </div>
      {position !== "top" && text}
    </div>
  );
}

/** Slider preview at a fixed 60%. */
function SliderBody({ vertical, icon, label }: { vertical: boolean; icon: Icon | null; label: string }) {
  const track = (
    <div
      className={cx("relative overflow-hidden rounded-full", vertical ? "w-[30cqmin] flex-1" : "h-[30cqmin] flex-1")}
      style={{ background: "color-mix(in srgb, currentColor 14%, transparent)" }}
    >
      <div
        className={cx("absolute", vertical ? "inset-x-0 bottom-0 h-[60%]" : "inset-y-0 left-0 w-[60%]")}
        style={{ background: "var(--accent)" }}
      />
    </div>
  );
  return (
    <div
      className={cx(
        "flex size-full items-center gap-[6cqmin] p-[10cqmin]",
        vertical ? "flex-col" : "flex-row",
      )}
    >
      {icon && <ControlIcon icon={icon} size="30cqmin" />}
      {track}
      {label && (
        <span className="max-w-full truncate text-center leading-tight font-medium" style={{ fontSize: "max(10px, 12cqmin)" }}>
          {label}
        </span>
      )}
    </div>
  );
}
