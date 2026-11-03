// end4-pC for Windows - where desktop cards go (each card is its own window, see desktop.js).

// Rough card heights, only used to stack cards that were never dragged.
const HEIGHT = { clock: 250, weather: 92, week: 90, greet: 90, media: 92, calendar: 270, todo: 150,
  notes: 170, timer: 100, worldclock: 140, resources: 130, text: 60 };
const GAP = 10, WIDTH = 310, TOP = 72, LEFT = 16;

/** Logical position of a card that has no saved spot: stacked in list order, wrapping into columns. */
export function defaultSlot(widgets, id, screenHeight) {
  let x = LEFT, y = TOP;
  for (const w of widgets) {
    const h = HEIGHT[w] ?? 120;
    if (y > TOP && y + h > screenHeight - 16) { x += WIDTH + GAP; y = TOP; }
    if (w === id) return { x, y };
    y += h + GAP;
  }
  return { x: LEFT, y: TOP };
}

/** True if a card's top-left corner (physical px) is on one of the monitors (Tauri's available_monitors). */
export const onScreen = (p, monitors) => monitors.some(m =>
  p.x >= m.position.x - 40 && p.x < m.position.x + m.size.width - 40 &&
  p.y >= m.position.y && p.y < m.position.y + m.size.height - 40);
