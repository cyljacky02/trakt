// AFK entry — isolated under .afk/, never the application's root package graph (ADR 0028).
// Delivered by create-afk-workflow@0.1.0.
import { runGates } from "./../scripts/gates.mjs";
import { runPoller } from "./../scripts/poller.mjs";

const [command] = process.argv.slice(2);
if (command === "gates") await runGates();
else await runPoller();
