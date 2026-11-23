# Interface direction

Create a self-contained, full-viewport application in a polished Gruvbox Dark operator-console aesthetic. Build one shallow utility rail across the top, one shallow status rail across the bottom, and a tightly fitted asymmetric mosaic of work panes between them. Lead with one dominant workspace and one broad ledger, stream, or secondary workspace; keep navigation and contextual regions visibly subordinate. Pane edges align into a strong two-dimensional grid separated by hairline gutters. This is refined desktop operations software, not a generic website placed inside a dark frame and not a literal imitation of crude stock Windows widgets.

Use warm charcoal backgrounds, cream foreground text, muted secondary text, and the recognizable Gruvbox red, green, yellow, blue, purple, aqua, and orange accents. Keep pane bodies within one quiet neutral family. Use the bright colors to locate regions and communicate meaning: chiefly on slim pane title strips, selected states, small signals, chart marks, and important values. Do not color large command faces or large areas of pane content.

## Colored pane chrome

Cap every primary pane with the same very shallow colored title strip. Treat it as a thin planar metal band with straight parallel edges, not a rounded tube, glossy capsule, button, or inflated plastic bar. Use square corners with no border radius. Keep its height close to one compact text row and use tight horizontal padding. Begin each title with a compact rectangular dark index plaque, align all title text on one baseline, and let optional metadata sit quietly at the far edge.

Give neighboring panes distinct muted Gruvbox instrument hues. Construct each strip from three restrained vertical zones: a narrow lighter upper rim, a broad middle held close to the base hue, and a narrow darker lower rim. Keep the lightness change modest and place the strongest highlight at the uppermost edge, never across the center of the face. The middle must remain visually flat. Add only a faint translucent inner highlight along the top, a restrained dark inner shade along the bottom, and a compact soft shadow immediately below the strip.

Do not use a continuous pale-to-dark gradient that makes the whole strip appear convex. Do not use a bright pastel upper half, centered gloss band, radial gradient, highlight bloom, rounded end, pill silhouette, thick bevel, double outline, or deep drop shadow. Do not allow the colored chrome to resemble the command buttons. Across different hues, preserve identical height, highlight placement, edge treatment, and shadow depth so the pane grid reads as one coherent instrument frame.

Use tiny crisp monospaced typography, near-solid leading, shallow chrome, compact rows, strict column alignment, clipped overflow, and fine separators. Density must come from useful domain information and disciplined geometry rather than illegibility or ornamental clutter. Every visible datum and control must plausibly help operate this particular product. Do not repeat summaries or invent filler merely to occupy space.

## Command controls

All ordinary command buttons share one restrained construction. Use a compact content-sized dark face approximately one text row high, tight horizontal padding, a neutral one-pixel outline, and square or nearly square corners. Shade the face with a subtle light-to-dark vertical gradient. Add a faint translucent highlight just inside the upper edge, restrained darker inset shading along the lower edge, and a short soft shadow centered directly beneath the face. The result should look gently raised and clean, not chunky, pixel-stacked, or heavily beveled.

Do not use asymmetric pale top-and-left borders against black right-and-bottom borders. Do not use a detached hard one-pixel offset shadow, multiple visible outline tiers, large vertical padding, broad minimum widths, or a shadow displaced sideways. Do not stretch ordinary commands into ribbons merely to fill a toolbar. Keep discrete commands content-sized and separated by a narrow consistent gap. Do not present browser-default controls or oversized form chrome.

On hover, a command may brighten slightly and instantaneously. On press, move its face down by one pixel only, collapse the lower cast shadow, and strengthen the dark inset shading so the face feels mechanically depressed. Never move a pressed command sideways. Never animate or transition the response. Keep ordinary, primary, destructive, and toggle commands materially consistent; communicate state with the label and nearby semantic indicators instead of enlarged or brightly recolored keys.

Tabs are not command buttons. Render them as contiguous flat segments in a shallow dark tab well, separated by crisp rules and without the ordinary button bevel, hover brightening, press movement, or cast shadow. Mark the active tab with a narrow accent line at its upper edge, a slightly deeper face, and a compact lower shelf that visually joins it to the content plane. Unselected tabs remain quiet and flat.

Inputs, text areas, and selectors use compact recessed dark fields, fine neutral borders, modest padding, and visible labels. Their depth must be quieter than the command buttons and must not introduce a second, harsher bevel language. Disabled controls remain legible but subdued. Use a clear high-contrast treatment on every interactive control.

## Information surfaces

Render tables and event streams as rigorous ledgers with aligned columns, thin cell rules, clipped overflow, and sticky headings where useful. Render charts directly on ruled dark plotting fields with simple luminous marks and restrained shading. Render meters as narrow recessed tracks with polished bright fills. Keep semantic colors consistent for selected, nominal, caution, critical, and secondary states.

Keep markup and framing flat. A wrapper must contribute unique layout, scrolling, clipping, positioning, or styling behavior to justify its existence. Let primary content occupy its pane directly. Avoid cards inside cards, controls inside unnecessary bordered boxes, and ornamental containers that split the viewer's attention.

Make responsive behavior change information topology rather than scaling the interface into a miniature. At broad widths, show the dominant workspace, broad ledger, narrow index, and narrow contextual inspector when useful. At a middle width, remove the inspector first and redistribute its space. At a narrow width, remove the index, simplify the utility and status rails, and hide low-priority ledger columns. Preserve readable type, compact controls, and a useful vertical relationship between the dominant workspace and its secondary surface. Keep the document fixed to the viewport; only data-heavy pane bodies scroll.

Use semantic landmarks and real compact interactions. Do not use pills, translucent glass, rounded modern cards, floating shadow islands, giant headings, oversized controls, animation, or transitions. Deliver one HTML document with embedded CSS and JavaScript and no external dependencies.
