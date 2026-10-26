# Piwas / Dicle — The living courtyard

Direction 02 is an interactive design proposal. Dicle is a working name. Product images, prices, measurements and sample business data are illustrative. No commerce service is connected.

Run from this directory:

```sh
python3 -m http.server 48763 --bind 127.0.0.1
```

Open [the design studio](http://localhost:48763/studio.html). Use the screen buttons for the storefront, set, room planner and admin. Desktop/mobile buttons control the presentation. The studio defaults to mobile in a narrow panel. `index.html` provides the full responsive page; serve over HTTP for the studio navigation.

Try the seating’s textile hotspot, the round room-planner control, size/fill options, favourite, sample basket and room measurements. Motion respects the device preference and can also be paused.

See [design notes](DESIGN-NOTES.md) for the current visual system, validation and limitations. Font licences are saved with the fonts. Generated/reference imagery must be replaced with actual permissioned product media before sale.

- [Development plan](../../docs/plans/COMMERCE-PLAN.md)
- [Research and sources](../../docs/research/commerce-research.md)
- [Image briefs](IMAGE-PROMPT-V2.txt)
