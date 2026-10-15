import "react";

declare module "react" {
  // Allow theme variables (`--bg`, `--radius`, ...) in inline styles.
  interface CSSProperties {
    [variable: `--${string}`]: string | number | undefined;
  }
}
