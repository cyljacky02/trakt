# AFK scheduler recipe (linux) — invokes the isolated AFK entry under .afk/.
# Delivered by create-afk-workflow@0.1.0.
exec node ".afk/runner/afk.mjs" poll
