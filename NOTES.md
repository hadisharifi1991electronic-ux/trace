
- Design reuse (Edit > Copy block layout…, `trace reuse`): a block laid
  out once (a channel, a supply) is copied onto another instance of it.
  Take the block's parts, pick the copy's counterpart of its anchor part,
  and Trace matches the rest through their nets (falling back to the
  designator offset, R12 -> R22, for parts joined only by ground or a
  supply), places them the same way about the anchor and copies the
  block's tracks, arcs and vias with the copy's nets. Shared nets copy
  only the stubs from the block's pads and their vias. The copy's own
  routing in its area is taken up first (unless kept). One undo step.
- Net ties (Edit > Net tie: mark selected parts, `trace nettie`): a
  part's own copper may join its nets (a star ground, a Kelvin joint)
  without the rule check calling it a short. Parts named NT…, footprints
  named NetTie, and parts with pads of different nets laid one on another
  (as Allegro draws them) are net ties without marking.

