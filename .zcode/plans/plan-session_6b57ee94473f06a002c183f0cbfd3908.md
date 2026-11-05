## Speech-bubble picker with a header-anchored clamp

**Positioning** — `table_header_menu_overlay` (view.rs): the panel targets a fixed offset from the header — left edge at `at.x + 6`, vertically centered on the header band (`at.y + 10 - MENU_H/2`, the anchor being the band top). The **clamp stays**: if the target position would push the panel off the window, it's clamped back inside (as today); a partially-offscreen render is not wanted after all.

**Speech-bubble tail** — a small left-pointing triangle connecting the panel to the header, drawn only when the panel is NOT clamped (the tail would point at nothing / the wrong spot otherwise):
- A tiny canvas widget (`PickerTail { tip: Point, color: Color }`) in the overlay stack between the backdrop and the panel, drawing a filled triangle whose tip touches the header band (at `at.x`, vertically centered on the band) and whose base merges into the panel's left edge. Filled with the panel's background; drawn before the panel so the seam is covered.
- The group picker shares the overlay, so it gets the tail for free.
- Tail placement follows the SAME clamped x/y as the panel: when clamped, only the panel moves — no tail.

**Implementation notes**: the tail geometry is a pure helper (`tail_triangle(at, panel_y) -> [Point; 3]`) unit-tested (tip on the band line, base on the panel edge, symmetric); `clamped` is a simple boolean computed from the same comparison the clamp uses. No changes outside `view.rs`; existing picker tests stay green.