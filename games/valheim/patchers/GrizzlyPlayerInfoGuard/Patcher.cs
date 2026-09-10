using System.Collections.Generic;
using System.Linq;
using Mono.Cecil;
using Mono.Cecil.Cil;

namespace GrizzlyPlayerInfoGuard
{
    /// <summary>
    /// BepInEx preloader patcher that keeps a null string from aborting Valheim's
    /// peer handshake.
    /// </summary>
    /// <remarks>
    /// Valheim 1.0 leaves <c>PlayerInfo.m_serverAssignedDisplayName</c> null on a
    /// Steam-only dedicated server, because the field is assigned from a PlayFab
    /// session the server only establishes when crossplay is on. <c>WritePlayerInfo</c>
    /// writes it unguarded, so <c>ZPackage.Write(string)</c> reaches
    /// <c>BinaryWriter.Write(null)</c> and throws. That throw happens inside
    /// <c>ZNet.RPC_PeerInfo</c>, which abandons the handshake before the peer is
    /// registered for world streaming — the joining client sits on the loading
    /// screen forever. Crossplay populates the field but moves connections onto the
    /// PlayFab relay, which breaks the direct IP:port joining this platform hands out.
    ///
    /// The guard goes on <c>ZPackage.Write(string)</c> rather than on the one field,
    /// because every null string reaching that method is fatal the same way and
    /// Valheim writes many of them. A null currently throws, so turning it into an
    /// empty string cannot change the behaviour of anything that works today.
    /// </remarks>
    public static class Patcher
    {
        /// <summary>Assemblies the BepInEx preloader hands to <see cref="Patch"/>.</summary>
        public static IEnumerable<string> TargetDLLs { get; } = new[] { "assembly_valheim.dll" };

        /// <summary>Rewrites <c>ZPackage.Write(string)</c> to substitute an empty string for null.</summary>
        public static void Patch(AssemblyDefinition assembly)
        {
            var zpackage = assembly.MainModule.GetType("ZPackage");
            if (zpackage == null)
            {
                return;
            }

            var write = zpackage.Methods.FirstOrDefault(method =>
                method.Name == "Write"
                && method.HasBody
                && method.Parameters.Count == 1
                && method.Parameters[0].ParameterType.FullName == "System.String");
            if (write == null || write.Body.Instructions.Count == 0)
            {
                return;
            }

            var data = write.Parameters[0];
            var body = write.Body;
            var il = body.GetILProcessor();
            var original = body.Instructions[0];

            // if (data == null) { data = ""; }
            il.InsertBefore(original, il.Create(OpCodes.Ldarg, data));
            il.InsertBefore(original, il.Create(OpCodes.Brtrue, original));
            il.InsertBefore(original, il.Create(OpCodes.Ldstr, string.Empty));
            il.InsertBefore(original, il.Create(OpCodes.Starg, data));
        }
    }
}
