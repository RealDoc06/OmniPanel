// Lucide icons (UI chrome) and deck control icons (Lucide, emoji or imported image).
import type { Icon } from "@omni/protocol";
import { iconMarkup, type IconName } from "../lib/icons";
import { useImageUrl } from "../lib/store";
import { cx } from "../lib/util";

/** Renders Lucide inner markup in the standard 24x24 stroke frame. */
export function Svg({ markup, className, strokeWidth = 2 }: { markup: string | undefined; className?: string; strokeWidth?: number }) {
  if (!markup) return null;
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className={cx("shrink-0", className)}
      dangerouslySetInnerHTML={{ __html: markup }}
    />
  );
}

/** A Lucide icon by (type-checked) name. Defaults to 16px. */
export function I({ name, className }: { name: IconName; className?: string }) {
  return <Svg markup={iconMarkup(name)} className={className ?? "size-4"} />;
}

/** A control's icon, `size` is any CSS length ("40cqmin", "20px"). */
export function ControlIcon({ icon, size }: { icon: Icon; size: string }) {
  const imageUrl = useImageUrl();
  const box = { width: size, height: size };
  switch (icon.type) {
    case "builtin":
      return (
        <span className="flex shrink-0" style={box}>
          <Svg markup={iconMarkup(icon.name)} className="size-full" />
        </span>
      );
    case "emoji":
      return (
        <span
          className="flex shrink-0 items-center justify-center leading-none"
          style={{ ...box, fontSize: `calc(${size} * 0.82)` }}
          aria-hidden
        >
          {icon.value}
        </span>
      );
    case "image":
      return <img src={imageUrl(icon.id)} alt="" draggable={false} className="shrink-0 object-contain" style={box} />;
  }
}
