# games/valheim/patchers

`GrizzlyPlayerInfoGuard.dll` — a BepInEx preloader patcher baked into the Valheim image and re-placed into BepInEx's `patchers/` directory on every boot by `valheim-pre-server.sh`. It rewrites `ZPackage.Write(string)` to substitute an empty string for null, which is what keeps a Steam-only server's peer handshake from aborting. What it fixes and why it is a patcher rather than a plugin: [ADR-038](../../../docs/decisions/038-valheim-null-player-info-guard.md).

**It is committed as a built binary, and its C# source is not in this repo.** The gate fails closed on any file in a language it has no adapter for, and one `.cs` file blocks the whole pipeline — including the signing step the bot's admission policy requires. Vendoring the output keeps the gate meaningful for the Rust that is the product, at the cost of the source not living here. See [ADR-041](../../../docs/decisions/041-vendor-the-valheim-patcher.md).

## Rebuilding it

The source is `games/valheim/patchers/GrizzlyPlayerInfoGuard/` as of commit `635c7a5` — `git show 635c7a5:games/valheim/patchers/GrizzlyPlayerInfoGuard/Patcher.cs` and the `.csproj` beside it. Restore that directory, then build with the same pinned SDK image the removed Dockerfile stage used:

```sh
docker run --rm \
  -v "$PWD/games/valheim/patchers/GrizzlyPlayerInfoGuard:/src:ro" \
  -v /tmp/patcher-out:/out -w /build \
  mcr.microsoft.com/dotnet/sdk@sha256:4ea6fe75dd36706bb6d8c3c293d4c4315840f5d76ea28ac97def77e3ec487fa5 \
  sh -c 'cp -r /src/. /build/ && dotnet publish -c Release -o /publish \
         && cp /publish/GrizzlyPlayerInfoGuard.dll /out/'
```

Copy the result over `GrizzlyPlayerInfoGuard.dll` here and delete the source directory again before committing — a `.cs` file left in the tree fails the gate and blocks every deploy, not just this image's.

The project targets `netstandard2.0` and references Mono.Cecil 0.11.5 with `ExcludeAssets="runtime"`, so the publish output is this one assembly: BepInEx loads the preloader against its own Mono.Cecil in `BepInEx/core`, and a second copy beside the patcher would put two Cecils in the process.

## Checking what shipped

The assembly's string literals are UTF-16LE in the metadata's user-string heap, so a plain `grep` finds nothing. To confirm a build is the patcher you think it is:

```sh
python3 -c "d=open('GrizzlyPlayerInfoGuard.dll','rb').read(); \
  print(all(s.encode('utf-16-le') in d for s in ['assembly_valheim.dll','ZPackage']))"
```
