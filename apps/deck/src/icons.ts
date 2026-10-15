// Lucide icon markup (24x24 viewBox, stroke = currentColor) used by the deck chrome.

/** Wraps inner SVG markup in the shared 24x24 stroke-icon shell, filling its parent box. */
export function svg(markup: string): string {
  return (
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%">' +
    markup +
    "</svg>"
  );
}

export const ICONS = {
  gear: svg(
    "<path d='M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915'/><circle cx='12' cy='12' r='3'/>",
  ),
  backspace: svg("<path d='M20 5H9l-7 7 7 7h11a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2Z'/><path d='m18 9-6 6'/><path d='m12 9 6 6'/>"),
  edit: svg("<path d='M12 20h9'/><path d='M16.4 3.6a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z'/>"),
  close: svg("<path d='M18 6 6 18'/><path d='m6 6 12 12'/>"),
  maximize: svg("<path d='M8 3H5a2 2 0 0 0-2 2v3'/><path d='M21 8V5a2 2 0 0 0-2-2h-3'/><path d='M3 16v3a2 2 0 0 0 2 2h3'/><path d='M16 21h3a2 2 0 0 0 2-2v-3'/>"),
  minimize: svg("<path d='M8 3v3a2 2 0 0 1-2 2H3'/><path d='M21 8h-3a2 2 0 0 1-2-2V3'/><path d='M3 16h3a2 2 0 0 1 2 2v3'/><path d='M16 21v-3a2 2 0 0 1 2-2h3'/>"),
  reload: svg("<path d='M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8'/><path d='M21 3v5h-5'/>"),
  unlink: svg(
    "<path d='m18.84 12.25 1.72-1.71h-.02a5.004 5.004 0 0 0-.12-7.07 5.006 5.006 0 0 0-6.95 0l-1.72 1.71'/><path d='m5.17 11.75-1.71 1.71a5.004 5.004 0 0 0 .12 7.07 5.006 5.006 0 0 0 6.95 0l1.71-1.71'/><line x1='8' x2='8' y1='2' y2='5'/><line x1='2' x2='5' y1='8' y2='8'/><line x1='16' x2='16' y1='19' y2='22'/><line x1='19' x2='22' y1='16' y2='16'/>",
  ),
};
