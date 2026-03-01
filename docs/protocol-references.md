**1) Theoretical protocol resources (docs/spec/wikis)**
- **Official source of truth**: [Mojang/bedrock-protocol-docs](https://github.com/Mojang/bedrock-protocol-docs) (README states current release + network version; updated Jan 2026).
- **Parsed/diff-friendly mirror of official docs**: [CloudburstMC/protocol-docs](https://github.com/CloudburstMC/protocol-docs).
- **Community protocol wiki**: [Bedrock Wiki: Bedrock Protocol](https://wiki.bedrock.dev/servers/bedrock) (good practical notes on login stages, compression IDs, packet header).
- **Community-maintained protocol wiki project**: [bedrock-crustaceans/protocol-wiki](https://github.com/bedrock-crustaceans/protocol-wiki) and [site](https://bedrock-crustaceans.github.io/protocol-wiki/).
- **Historical/legacy reference (use cautiously)**: [Minecraft Wiki: Bedrock Edition protocol](https://minecraft.wiki/w/Bedrock_Edition_protocol) (explicitly documents older protocol versions like 1.16.220).

**2) Login sequence / connection establishment deep-dive resources**
- [Minecraft Wiki: Bedrock Login Sequence](https://minecraft.wiki/w/Minecraft_Wiki:Projects/wiki.vg_merge/Bedrock_Login_Sequence) (JWT chain validation, handshake order, XBL flow details).
- [Bedrock Wiki login process section](https://wiki.bedrock.dev/servers/bedrock) (RequestNetworkSettings -> NetworkSettings -> Login -> Handshake -> resource pack flow -> StartGame).
- [PrismarineJS bedrock-protocol API docs](https://raw.githubusercontent.com/PrismarineJS/bedrock-protocol/master/docs/API.md) (event order: connect/login/join/spawn, handshake/encryption lifecycle).
- [gophertunnel login package docs](https://pkg.go.dev/github.com/sandertv/gophertunnel/minecraft/protocol/login) and [request.go](https://github.com/Sandertv/gophertunnel/blob/master/minecraft/protocol/login/request.go) (JWT parse/verify, auth result and key usage).

**3) Packet disassembly / structure / encryption / compression resources**
- [Mojang official protocol docs](https://github.com/Mojang/bedrock-protocol-docs) (packet field trees/classes/enums).
- [gophertunnel packet package](https://pkg.go.dev/github.com/sandertv/gophertunnel/minecraft/protocol/packet) (packet encode/decode, compressed batches, optional encryption).
- [pmmp/BedrockProtocol README](https://raw.githubusercontent.com/pmmp/BedrockProtocol/master/README.md) (PacketBatch decode/encode examples; clearly states crypto/compression out-of-scope).
- [PMMP internals: implementing new MC version support](https://doc.pmmp.io/en/rtfd/developers/internals-docs/updating-minecraft-protocol.html) (reverse-engineering workflow: BDS symbols, objdump, Frida trace, manual packet structure updates).
- [pmmp/BedrockProtocolDumper README](https://raw.githubusercontent.com/pmmp/BedrockProtocolDumper/master/README.md) (historical RE tool; now deprecated in favor of bds-modding-devkit).
- Adjacent transport RE: [df-mc/nethernet-spec](https://raw.githubusercontent.com/df-mc/nethernet-spec/main/README.md) (WebRTC-based transport reverse engineering notes, encryption/checksum details).

**4) Practical open-source implementations (frameworks/packages)**
Maintenance labels below are **inferred** from latest default-branch commit timestamps + release activity (as of Mar 2026).

| Project | Primary language | Maintenance status (2026) | Packet/protocol capabilities | Repo |
|---|---|---|---|---|
| CloudburstMC/Protocol | Java | **Very active** (latest commit Feb 9, 2026: [commit](https://github.com/CloudburstMC/Protocol/commit/57f9846551d4bfc392d74846db098059bf5529d5)) | Bedrock protocol library (`bedrock-codec`, `bedrock-connection`), multi-version support, parser/codec foundation for proxies/servers | https://github.com/CloudburstMC/Protocol |
| PrismarineJS/bedrock-protocol | JavaScript | **Very active** (Feb 11, 2026: [commit](https://github.com/PrismarineJS/bedrock-protocol/commit/587bccd4e08e37592012cf5299ad01e67bc1b0bb)) | Parse/serialize packets as JS objects, client/server, XBL auth, encryption, MITM relay/proxy hooks | https://github.com/PrismarineJS/bedrock-protocol |
| Sandertv/gophertunnel | Go | **Very active** (Feb 15, 2026: [commit](https://github.com/Sandertv/gophertunnel/commit/4f4f52e4a6caa389471e1b1f7b40f7e65a457475)) | Full Bedrock connection stack, JWT login parsing/verification, packet encode/decode, compression and optional encryption handling, MITM example | https://github.com/Sandertv/gophertunnel |
| pmmp/BedrockProtocol | PHP | **Very active** (Feb 15, 2026: [commit](https://github.com/pmmp/BedrockProtocol/commit/34d9d6162f39de898b03fa436177cbbb8fabfed4)) | Detailed packet classes + PacketBatch decoding/encoding; explicitly excludes JWT/auth/encryption/compression | https://github.com/pmmp/BedrockProtocol |
| pmmp/PocketMine-MP | PHP (+ C/C++) | **Very active** (Feb 25, 2026: [commit](https://github.com/pmmp/PocketMine-MP/commit/4f563d39044b06d1e4724032369d67f347dd3b86)) | Full Bedrock server implementation, plugin API, frequent protocol upgrades, protocol/data tooling ecosystem | https://github.com/pmmp/PocketMine-MP |
| WaterdogPE/WaterdogPE | Java | **Very active** (Feb 13, 2026: [commit](https://github.com/WaterdogPE/WaterdogPE/commit/eff96be532a9f7f730ab25dc6cdf24737cde3d7f)) | Bedrock proxy software, routing/transfer between backends, plugin API, built on Cloudburst Protocol | https://github.com/WaterdogPE/WaterdogPE |
| GeyserMC/Geyser | Java | **Very active** (Feb 26, 2026: [commit](https://github.com/GeyserMC/Geyser/commit/bccae77ab8d0a362af9f212d1c285b2b28207250)) | Bedrock<->Java protocol translation bridge/proxy, broad version mediation, packet translation pipeline | https://github.com/GeyserMC/Geyser |
| CloudburstMC/Nukkit | Java | **Very active** (Feb 15, 2026: [commit](https://github.com/CloudburstMC/Nukkit/commit/8a1a9f25b08db2ee8808f0a0021e48075b33dfd4)) | Bedrock server software with plugin API; practical server-side protocol management | https://github.com/CloudburstMC/Nukkit |
| df-mc/dragonfly | Go | **Very active** (Feb 28, 2026: [commit](https://github.com/df-mc/dragonfly/commit/e0ce8ff28eef6d9982906b8b7079511cb0552720)) | Asynchronous Bedrock server framework/library, extensible architecture for protocol-driven server behavior | https://github.com/df-mc/dragonfly |
| PowerNukkitX/PowerNukkitX | Java | **Very active** (Mar 1, 2026: [commit](https://github.com/PowerNukkitX/PowerNukkitX/commit/96c743dbaa7e535f9c245933531b2820e30d06e6)) | Feature-rich Bedrock server stack (custom items/blocks/entities, command support), active protocol tracking | https://github.com/PowerNukkitX/PowerNukkitX |

**5) Synthesis**
- For **theory/spec correctness**, prioritize: Mojang docs -> Cloudburst parsed docs -> Bedrock Wiki/Minecraft Wiki pages.
- For **login + crypto implementation reality**, prioritize: gophertunnel + Prismarine + PMMP internals docs.
- For **production-grade practical tooling**, current leaders are: Geyser, WaterdogPE, PocketMine-MP, Dragonfly, Cloudburst Protocol, gophertunnel.
- I found relatively few standalone blog-style deep dives; the highest-quality material is concentrated in **wikis + source code + maintainer docs/issues**.