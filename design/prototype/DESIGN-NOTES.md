# The living courtyard — direction 02

A design proposal for the Piwas şark köşesi shop. Dicle remains a working name. Product media, prices, measurements and business data are illustrative; this is a local interactive concept.

## Visual system

- Palette: forest `#153D31`, pale pistachio `#E4ECB3`, pomegranate `#933B4A`, mineral green `#E9EDDF`, ink `#183C32`, leaf `#B1C09C`. The textile and courtyard photograph carry the richest colour; commerce controls use calmer, legible surfaces.
- Type: locally hosted Bricolage Grotesque for the wordmark and large sculptural headlines; DM Sans for controls and descriptive copy. Font licences are saved under `assets/fonts/`. Arabic uses a system fallback; production needs a deliberately paired Arabic family and native-reader review.
- Opening: an immersive basalt courtyard with monumental left-aligned type and a visible seating silhouette. Mobile recomposes the scene vertically so the product remains visible below the invitation.
- Collection: a transparent seating cutout overlaps the word “together.” The product becomes the composition, with one clear configuration action.
- Room planning: a CSS-built isometric room invites exploration, followed by the usable rectangular dimension planner. It is not a demonstration of real AR.
- Product: an arched image, explicit size and fill choices, custom icons, and a clear sample basket.
- Icons: a consistent 24px SVG family for seating, layers, room, tea, delivery, favourites, shopping, navigation and admin. The abstract four-loop brand mark is a design proposal, not a claim about a particular traditional symbol.

## Motion

The opening has one coordinated camera/text entrance. Pointer movement produces a small scene shift. The room-planner seal rotates slowly and can be paused. Opening the textile detail, saving a favourite, changing views, hovering the room model and updating the footprint each show a relevant response. There is no scroll hijacking, autoplay video or carousel. `prefers-reduced-motion` disables animations and transitions, and the decorative-motion control exposes its state.

## Review critique

The visual emphasis belongs to the courtyard, oversized typography and real seating silhouette. Motifs are limited to the lower woven edge and a few purposeful icons. The product and admin views remain more restrained so that choices and operational information stay easy to read. The English display word and some explanatory/admin copy still need complete localisation. Motion and atmosphere require browser review; a screenshot only captures a single state.

## Prototype boundaries

Storefront, size/fill configuration, local basket, local favourite, dimension planner, illustrative admin tabs, system/light/dark preference and seven-language core copy. No checkout, emails, registration, live inventory, room uploads or AR. Translations are drafts. The planner checks a rectangle with sample dimensions and does not assess doors, obstructions or tolerances.

## Current preview checks

- Desktop at 1280px and phone at 390px visually inspected; no page-level horizontal overflow in checked English, French and Arabic views.
- Textile disclosure, covers-only selection and €189 sample basket verified through visible UI.
- Rotated 70 × 180cm footprint correctly reports too large for a 320 × 150cm rectangle.
- Arabic RTL/dark and long French phone headline inspected.
- Reduced-motion emulation changes the motion control to paused and the orbit’s computed animation to `none`; emulation restored after checking.
- Browser error log was empty during these checks.

These checks are prototype smoke checks, not production accessibility, localisation, performance, security or commerce acceptance.

## Media provenance

- `assets/reference-only.jpg`: user-provided external model example, not Piwas stock.
- `assets/courtyard-v2.png`: generated editorial courtyard inspired by the reference. It is not a real place or product photograph.
- `assets/seating-cutout-v2.png`: generated transparent seating illustration inspired by the reference. It is not a measured or verified product image.
- `assets/concept-room.png`: original generated room retained with the earlier direction.
- New generation briefs are saved in `IMAGE-PROMPT-V2.txt`. Replace all concept/reference media with permissioned actual product media before sale.

## Review

Use `studio.html` for storefront, set, planner and admin screens, with desktop/mobile controls. It defaults to a phone preview in a narrow side panel. The name, final product specification and production design remain open for discussion.
