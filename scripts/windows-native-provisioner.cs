// Test-only Windows provisioner. No production authority or platform fallback.
// Loaded only from the provisioner-controlled checkout before test accounts run.
using System;
using System.IO;
using System.Text;
using System.Linq;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Security.Cryptography;
using System.Text.Json;

namespace Surge.NativeFixtures {
    public static class Contract {
        public const string Prefix = "state_home::privileged_fixture_tests::";
        public static readonly string[] Filters = {
            Prefix + "admin_default_owner_creation",
            Prefix + "standard_user_below_admin_ancestor",
            Prefix + "provision_foreign_owner_fixture",
            Prefix + "foreign_owner_refuses_before_initialization"
        };
        public static bool Elevated(string filter) {
            if (!Filters.Contains(filter, StringComparer.Ordinal)) throw new InvalidDataException("Unknown literal filter");
            return filter == Filters[0] || filter == Filters[2];
        }
        public static string Quote(string value) {
            if (value.IndexOf('\0') >= 0) throw new InvalidDataException("NUL argument");
            var output = new StringBuilder("\""); int slashes = 0;
            foreach (char c in value) {
                if (c == '\\') { slashes++; continue; }
                output.Append('\\', c == '"' ? slashes * 2 + 1 : slashes);
                output.Append(c); slashes = 0;
            }
            return output.Append('\\', slashes * 2).Append('"').ToString();
        }
        public static string Command(string executable, string filter) {
            Elevated(filter);
            string value = String.Join(" ", new[] { executable, "--ignored", "--exact", filter, "--nocapture", "--test-threads=1" }.Select(Quote));
            if (value.Length >= 1024) throw new InvalidDataException("WithToken command line exceeds documented limit");
            return value;
        }
        public static string EnvironmentBlock(IDictionary<string,string> values) {
            var sorted = new SortedDictionary<string,string>(StringComparer.OrdinalIgnoreCase);
            foreach (var pair in values) {
                if (String.IsNullOrEmpty(pair.Key) || pair.Key.IndexOfAny(new[] {'=', '\0'}) >= 0 || pair.Value.IndexOf('\0') >= 0)
                    throw new InvalidDataException("Invalid environment entry");
                if (sorted.ContainsKey(pair.Key)) throw new InvalidDataException("Duplicate environment name");
                sorted.Add(pair.Key, pair.Value);
            }
            return String.Join("\0", sorted.Select(x => x.Key + "=" + x.Value)) + "\0\0";
        }
        public static string Hash(byte[] bytes) {
            using (var sha = SHA256.Create()) return Hex(sha.ComputeHash(bytes));
        }
        public static string Hex(byte[] bytes) { return BitConverter.ToString(bytes).Replace("-", "").ToLowerInvariant(); }
        public static void RequireHex(string value, int length) {
            if (value == null || value.Length != length || value.Any(c => !"0123456789abcdef".Contains(c))) throw new InvalidDataException("Invalid hexadecimal binding");
        }
        public static void RequireBinding(string expected,string actual,string phase) {
            if(!String.Equals(expected,actual,StringComparison.Ordinal)) throw new InvalidDataException(phase+" differs");
        }
        public static void RequireCapability(bool available,string capability) {
            if(!available) throw new InvalidOperationException("Setup failure: "+capability+" unavailable; no fallback");
        }
        public static void RequireImport(string name,ISet<string> reviewed,ISet<string> modules,IDictionary<string,string> hosts) {
            if(!reviewed.Contains(name)) throw new InvalidDataException("Unreviewed import: "+name);
            if(name.StartsWith("api-",StringComparison.Ordinal)) {
                if(!hosts.TryGetValue(name,out string host) || host!="ucrtbase.dll" || !modules.Contains(host)) throw new InvalidDataException("Unsupported OS API contract");
            } else if(!modules.Contains(name)) throw new InvalidDataException("Import lacks fixed system classification");
        }
        static void CleanupIdentity(JsonElement value) {
            using(var identity=Json(Encoding.UTF8.GetBytes(value.GetRawText()),"volume","file_id")) {
                string volume=identity.RootElement.GetProperty("volume").GetString();
                if(!UInt64.TryParse(volume,System.Globalization.NumberStyles.None,System.Globalization.CultureInfo.InvariantCulture,out ulong number) || volume!=number.ToString(System.Globalization.CultureInfo.InvariantCulture)) throw new InvalidDataException("Noncanonical cleanup volume");
                RequireHex(identity.RootElement.GetProperty("file_id").GetString(),32);
            }
        }
        public static void RequireCleanup(string filter,JsonElement facts) {
            Elevated(filter);
            if(filter==Filters[2]) {
                if(facts.TryGetProperty("cleanup",out _)) throw new InvalidDataException("Foreign setup cannot claim final cleanup");
                return;
            }
            string[] fields=filter==Filters[3]
                ? new[]{"sentinel_removed","foreign_home_removed","foreign_home_identity","control_home_removed","control_home_identity"}
                : filter==Filters[1] ? new[]{"runtime_home_removed","runtime_home_identity","sentinel_removed"}
                : new[]{"runtime_home_removed","runtime_home_identity"};
            if(!facts.TryGetProperty("cleanup",out var cleanup)) throw new InvalidDataException("Missing actor cleanup proof");
            using(var checkedCleanup=Json(Encoding.UTF8.GetBytes(cleanup.GetRawText()),fields)) {
                foreach(var property in checkedCleanup.RootElement.EnumerateObject()) {
                    if(property.Name.EndsWith("_identity",StringComparison.Ordinal)) CleanupIdentity(property.Value);
                    else if(property.Value.ValueKind!=JsonValueKind.True) throw new InvalidDataException("Actor cleanup incomplete");
                }
            }
        }
        public static JsonDocument Json(byte[] bytes, params string[] fields) {
            if (bytes.Length > 65536) throw new InvalidDataException("Receipt too large");
            var doc = JsonDocument.Parse(bytes, new JsonDocumentOptions { MaxDepth = 12 });
            if (doc.RootElement.ValueKind != JsonValueKind.Object) { doc.Dispose(); throw new InvalidDataException("Expected object"); }
            var seen = new HashSet<string>(StringComparer.Ordinal);
            foreach (var prop in doc.RootElement.EnumerateObject()) {
                if (!seen.Add(prop.Name) || !fields.Contains(prop.Name, StringComparer.Ordinal)) { doc.Dispose(); throw new InvalidDataException("Unknown or duplicate receipt field"); }
            }
            if (seen.Count != fields.Length) { doc.Dispose(); throw new InvalidDataException("Missing receipt field"); }
            try { UniqueMembers(doc.RootElement); return doc; } catch { doc.Dispose(); throw; }
        }
        static void UniqueMembers(JsonElement value) {
            if(value.ValueKind==JsonValueKind.Object) {
                var names=new HashSet<string>(StringComparer.Ordinal);
                foreach(var property in value.EnumerateObject()) {
                    if(!names.Add(property.Name)) throw new InvalidDataException("Duplicate nested JSON field");
                    UniqueMembers(property.Value);
                }
            } else if(value.ValueKind==JsonValueKind.Array) foreach(var item in value.EnumerateArray()) UniqueMembers(item);
        }
    }

    // Bounded PE import-name extraction; deliberately not a dependency loader.
    public static class PeImports {
        static ushort U16(byte[] b, int p) { Bounds(b,p,2); return BitConverter.ToUInt16(b,p); }
        static uint U32(byte[] b, int p) { Bounds(b,p,4); return BitConverter.ToUInt32(b,p); }
        static void Bounds(byte[] b, int p, int n) { if (p < 0 || n < 0 || p > b.Length - n) throw new InvalidDataException("PE bounds"); }
        sealed class Section { public uint Va, Size, Raw, RawSize; }
        static int Offset(byte[] b, List<Section> sections, uint rva, int size) {
            foreach (var s in sections) {
                if (rva >= s.Va && (ulong)rva + (uint)size <= (ulong)s.Va + s.RawSize) {
                    long p = (long)s.Raw + rva - s.Va;
                    if (p > Int32.MaxValue) break;
                    Bounds(b,(int)p,size); return (int)p;
                }
            }
            throw new InvalidDataException("Unmapped PE RVA");
        }
        static string Name(byte[] b, List<Section> sections, uint rva) {
            var chars = new List<byte>();
            for (uint i=0; i<260; i++) {
                byte c = b[Offset(b,sections,checked(rva+i),1)];
                if (c == 0) {
                    string name = Encoding.ASCII.GetString(chars.ToArray()).ToLowerInvariant();
                    if (name.Length == 0 || name.Any(x => !(x >= 'a' && x <= 'z') && !(x >= '0' && x <= '9') && x != '.' && x != '-' && x != '_')) throw new InvalidDataException("Unsafe import name");
                    return name;
                }
                if (c > 127) throw new InvalidDataException("Non-ASCII import");
                chars.Add(c);
            }
            throw new InvalidDataException("Unterminated import");
        }
        public static string[] Read(byte[] b) {
            if (b.Length > 512*1024*1024 || U16(b,0) != 0x5a4d) throw new InvalidDataException("PE DOS header");
            int pe = checked((int)U32(b,0x3c));
            if (U32(b,pe) != 0x4550 || U16(b,pe+4) != 0x8664) throw new InvalidDataException("Expected AMD64 PE");
            int count = U16(b,pe+6), optSize = U16(b,pe+20), opt=checked(pe+24);
            if (count == 0 || count > 96 || U16(b,opt) != 0x20b || optSize < 240 || U32(b,opt+108) < 14) throw new InvalidDataException("PE optional header");
            Bounds(b,opt,optSize); var sections = new List<Section>();
            for (int i=0; i<count; i++) {
                int s=checked(opt+optSize+i*40); Bounds(b,s,40);
                sections.Add(new Section { Va=U32(b,s+12), Size=U32(b,s+8), RawSize=U32(b,s+16), Raw=U32(b,s+20) });
            }
            var names = new HashSet<string>(StringComparer.Ordinal);
            foreach (int dir in new[] {1,13}) {
                uint rva=U32(b,opt+112+dir*8), size=U32(b,opt+116+dir*8);
                if (rva == 0 && size == 0) continue;
                int width=dir == 1 ? 20 : 32;
                if (rva == 0 || size < width || size > 1048576) throw new InvalidDataException("PE import directory");
                bool terminated=false;
                for (uint pos=0; pos+(uint)width<=size; pos+=(uint)width) {
                    int p=Offset(b,sections,checked(rva+pos),width);
                    bool zero=true; for (int k=0;k<width;k++) zero &= b[p+k] == 0;
                    if (zero) { terminated=true; break; }
                    if (dir == 13 && U32(b,p) != 1) throw new InvalidDataException("Unsupported delay import addressing");
                    names.Add(Name(b,sections,U32(b,p+(dir==1 ? 12 : 4))));
                }
                if (!terminated) throw new InvalidDataException("Unterminated import table");
            }
            return names.OrderBy(x=>x,StringComparer.Ordinal).ToArray();
        }
    }

    internal static class Win {
        internal const uint ReadControl=0x20000, Sync=0x100000, ReadAttr=0x80, Traverse=0x20;
        internal const uint TokenQuery=8, TokenDuplicate=2, TokenAssign=1, TokenDefault=0x80, TokenPrivileges=0x20;
        [StructLayout(LayoutKind.Sequential)] internal struct Unicode { public ushort Length, MaximumLength; public IntPtr Buffer; }
        [StructLayout(LayoutKind.Sequential)] internal struct ObjectAttributes { public uint Length; public IntPtr Root; public IntPtr Name; public uint Attributes; public IntPtr Security; public IntPtr Quality; }
        [StructLayout(LayoutKind.Sequential)] internal struct IoStatus { public IntPtr Status; public UIntPtr Information; }
        [StructLayout(LayoutKind.Sequential)] internal struct SecurityAttributes { public uint Length; public IntPtr Descriptor; [MarshalAs(UnmanagedType.Bool)] public bool Inherit; }
        [StructLayout(LayoutKind.Sequential)] internal struct Startup { public uint cb; public IntPtr reserved, desktop, title; public uint x,y,xSize,ySize,xChars,yChars,fill,flags; public ushort show,reserved2; public IntPtr reservedBytes,input,output,error; }
        [StructLayout(LayoutKind.Sequential)] internal struct ProcessInfo { public IntPtr Process, Thread; public uint Pid, Tid; }
        [StructLayout(LayoutKind.Sequential)] internal struct Luid { public uint Low; public int High; }
        [StructLayout(LayoutKind.Sequential)] internal struct Privilege { public uint Count; public Luid Id; public uint Attributes; }
        [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] internal static extern IntPtr CreateFileW(string p,uint access,uint share,IntPtr security,uint disposition,uint flags,IntPtr template);
        [DllImport("ntdll.dll")] internal static extern int NtCreateFile(out IntPtr file,uint access,ref ObjectAttributes attributes,ref IoStatus status,IntPtr allocation,uint fileAttributes,uint shares,uint disposition,uint options,IntPtr ea,uint eaLength);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool CloseHandle(IntPtr h);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool GetFileInformationByHandleEx(IntPtr h,int kind,IntPtr output,uint size);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern uint GetFileType(IntPtr h);
        [DllImport("advapi32.dll", SetLastError=true)] internal static extern uint GetSecurityInfo(IntPtr h,int type,uint information,out IntPtr owner,IntPtr group,out IntPtr dacl,IntPtr sacl,out IntPtr descriptor);
        [DllImport("advapi32.dll")] internal static extern uint GetSecurityDescriptorLength(IntPtr descriptor);
        [DllImport("advapi32.dll",SetLastError=true,CharSet=CharSet.Unicode)] internal static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string s,uint revision,out IntPtr result,IntPtr size);
        [DllImport("kernel32.dll")] internal static extern IntPtr LocalFree(IntPtr pointer);
        [DllImport("kernel32.dll")] internal static extern IntPtr GetCurrentProcess();
        [DllImport("kernel32.dll")] internal static extern IntPtr GetCurrentThread();
        [DllImport("advapi32.dll",SetLastError=true)] internal static extern bool OpenProcessToken(IntPtr p,uint rights,out IntPtr token);
        [DllImport("advapi32.dll",SetLastError=true)] internal static extern bool OpenThreadToken(IntPtr p,uint rights,bool self,out IntPtr token);
        [DllImport("advapi32.dll",SetLastError=true)] internal static extern bool GetTokenInformation(IntPtr token,int kind,IntPtr output,uint size,out uint needed);
        [DllImport("advapi32.dll",SetLastError=true)] internal static extern bool DuplicateTokenEx(IntPtr token,uint rights,IntPtr security,int level,int kind,out IntPtr duplicate);
        [DllImport("advapi32.dll",SetLastError=true)] internal static extern bool SetTokenInformation(IntPtr token,int kind,IntPtr value,uint size);
        [DllImport("advapi32.dll",SetLastError=true,CharSet=CharSet.Unicode)] internal static extern bool LookupPrivilegeValueW(string system,string name,out Luid id);
        [DllImport("advapi32.dll",SetLastError=true)] internal static extern bool AdjustTokenPrivileges(IntPtr token,bool all,ref Privilege value,uint size,IntPtr previous,IntPtr needed);
        [DllImport("advapi32.dll",SetLastError=true,CharSet=CharSet.Unicode)] internal static extern bool CreateProcessWithTokenW(IntPtr token,uint logon,string application,StringBuilder command,uint flags,IntPtr environment,string cwd,ref Startup startup,out ProcessInfo process);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern bool DuplicateHandle(IntPtr source,IntPtr handle,IntPtr target,out IntPtr result,uint access,bool inherit,uint options);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern uint ResumeThread(IntPtr thread);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern uint WaitForSingleObject(IntPtr h,uint timeout);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern bool GetExitCodeProcess(IntPtr p,out uint code);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern bool TerminateProcess(IntPtr p,uint code);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern bool SetFileInformationByHandle(IntPtr p,int kind,ref uint flags,uint size);
        [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] internal static extern IntPtr CreateEventW(IntPtr security,bool manual,bool initial,string name);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern bool SetEvent(IntPtr handle);
        [DllImport("kernel32.dll",SetLastError=true)] internal static extern bool GetProcessTimes(IntPtr process,out long creation,out long exit,out long kernel,out long user);
        [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] internal static extern bool QueryFullProcessImageNameW(IntPtr p,uint flags,StringBuilder name,ref uint size);
        [DllImport("psapi.dll",SetLastError=true)] internal static extern bool EnumProcessModulesEx(IntPtr process,IntPtr[] modules,uint bytes,out uint needed,uint filter);
        [DllImport("psapi.dll",SetLastError=true,CharSet=CharSet.Unicode)] internal static extern uint GetModuleFileNameExW(IntPtr process,IntPtr module,StringBuilder path,uint size);
        [DllImport("kernel32.dll",CharSet=CharSet.Unicode)] internal static extern uint GetSystemDirectoryW(StringBuilder value,uint size);
        [DllImport("kernel32.dll",CharSet=CharSet.Unicode)] internal static extern uint GetWindowsDirectoryW(StringBuilder value,uint size);
        [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] internal static extern bool GetVolumeInformationByHandleW(IntPtr file,IntPtr name,uint nameSize,out uint serial,out uint max,out uint flags,StringBuilder fs,uint fsSize);
        [DllImport("kernel32.dll",CharSet=CharSet.Unicode)] internal static extern uint GetDriveTypeW(string path);
        internal static void Check(bool result,string phase) { if (!result) throw new Win32Exception(Marshal.GetLastWin32Error(),phase); }
        internal static IntPtr Valid(IntPtr handle,string phase) { if (handle == IntPtr.Zero || handle == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error(),phase); return handle; }
    }

    public sealed class NativeHandle {
        internal IntPtr Value;
        public string Kind { get; private set; }
        internal NativeHandle(IntPtr value,string kind) { Value=Win.Valid(value,kind); Kind=kind; }
        public void Close() {
            IntPtr value=Value; Value=IntPtr.Zero;
            if (value != IntPtr.Zero) Win.Check(Win.CloseHandle(value),"CloseHandle "+Kind);
        }
    }
    public sealed class FileFact {
        public string volume { get; set; }
        public string file_id { get; set; }
        public string descriptor { get; set; }
        public bool directory { get; set; }
        public long length { get; set; }
        public int links { get; set; }
        public string Key { get { return volume+":"+file_id; } }
    }
    public sealed class HeldTree {
        internal readonly List<NativeHandle> Handles=new List<NativeHandle>();
        internal readonly List<string> Keys=new List<string>();
        internal readonly List<string> Names=new List<string>();
        public string Path { get; internal set; }
        internal NativeHandle Last { get { return Handles[Handles.Count-1]; } }
    }
    // Acceptance failure is sticky; delayed exit only resolves ownership, never the verdict.
    public sealed class SettlementState {
        public bool Failed { get; private set; }
        public bool ExitObserved { get; private set; }
        public bool Preserve { get; private set; }
        public void AcceptanceFailed() { Failed=true; }
        public void SettlementFailed() { Failed=true; Preserve=true; }
        public void ObserveExit() { ExitObserved=true; }
        public void RequireClosable() { if(!ExitObserved) throw new InvalidOperationException("Unobserved child cannot release ownership"); }
        public bool CleanupAllowed { get { return ExitObserved && !Preserve; } }
    }
    public static class CleanupBatch {
        public static void Run(IEnumerable<KeyValuePair<string,Action>> actions,Action<string,Exception> failure) {
            foreach(var action in actions) {
                try { action.Value(); } catch(Exception error) { failure(action.Key,error); }
            }
        }
    }
    public sealed class Child {
        internal NativeHandle Process,Thread,Ready,Proceed,Cancel;
        internal readonly List<NativeHandle> Files=new List<NativeHandle>();
        public uint Pid { get; internal set; }
        public string Filter { get; internal set; }
        public string Actor { get; internal set; }
        public string Nonce { get; internal set; }
        public string Receipt { get; internal set; }
        public string ReadyReceipt { get; internal set; }
        public long CreationTime { get; internal set; }
        public SettlementState Settlement { get; } = new SettlementState();
        public bool ExitObserved { get { return Settlement.ExitObserved; } }
        public uint ExitCode { get; internal set; }
        public bool TimedOut { get; internal set; }
    }
    public sealed class Provisioner {
        const string Admin="S-1-5-32-544", SystemSid="S-1-5-18";
        const string Installer="S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
        readonly List<NativeHandle> handles=new List<NativeHandle>();
        readonly List<HeldTree> chains=new List<HeldTree>();
        readonly List<Child> children=new List<Child>();
        readonly Dictionary<string,string> protectedModules=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
        readonly Dictionary<string,string> systemModuleFacts=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
        readonly HashSet<string> allowedSystemModules=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        readonly List<object> resources=new List<object>();
        readonly object failedObservationGate=new object();
        NativeHandle token;
        HeldTree launch;
        string candidateHash, executable;
        public string ActorSid { get; private set; }
        public string StandardSid { get; private set; }
        public string ForeignSid { get; private set; }
        public string SystemDirectory { get; private set; }
        public string WindowsDirectory { get; private set; }
        public string CandidateHash { get { return candidateHash; } }
        public string Executable { get { return executable; } }
        public readonly List<string> Errors=new List<string>();
        public bool PreserveResources { get; private set; }
        public string FailAfter { get; set; }
        public object[] Resources { get { return resources.ToArray(); } }
        public Child[] Children { get { return children.ToArray(); } }
        void Point(string phase) { if (FailAfter == phase) throw new InvalidOperationException("Injected failure: "+phase); }
        NativeHandle Own(IntPtr raw,string kind) {
            var h=new NativeHandle(raw,kind); handles.Add(h);
            resources.Add(new { kind="handle", name=kind, acquired=true }); Point("acquire:"+kind); return h;
        }
        static string DirectoryFromSdk(bool system) {
            var result=new StringBuilder(32768);
            uint length=system ? Win.GetSystemDirectoryW(result,32768) : Win.GetWindowsDirectoryW(result,32768);
            if (length==0 || length>=32768) throw new InvalidDataException("SDK system directory unavailable");
            return result.ToString();
        }
        static byte[] TokenBytes(NativeHandle t,int kind) {
            uint size; Win.GetTokenInformation(t.Value,kind,IntPtr.Zero,0,out size);
            if (Marshal.GetLastWin32Error()!=122 || size<4 || size>65536) throw new InvalidDataException("Token size query");
            IntPtr output=Marshal.AllocHGlobal((int)size);
            try { Win.Check(Win.GetTokenInformation(t.Value,kind,output,size,out size),"GetTokenInformation");
                byte[] bytes=new byte[size]; Marshal.Copy(output,bytes,0,bytes.Length); return bytes;
            } finally { Marshal.FreeHGlobal(output); }
        }
        static T WithToken<T>(NativeHandle t,int kind,Func<IntPtr,uint,T> read) {
            uint size; Win.GetTokenInformation(t.Value,kind,IntPtr.Zero,0,out size);
            if (Marshal.GetLastWin32Error()!=122 || size<4 || size>65536) throw new InvalidDataException("Token size query");
            IntPtr output=Marshal.AllocHGlobal((int)size);
            try { uint returned; Win.Check(Win.GetTokenInformation(t.Value,kind,output,size,out returned),"GetTokenInformation");
                if (returned>size) throw new InvalidDataException("Token returned length"); return read(output,returned);
            } finally { Marshal.FreeHGlobal(output); }
        }
        static string SidAt(IntPtr allocation,uint length,IntPtr sid) {
            long offset=sid.ToInt64()-allocation.ToInt64();
            if (length<8 || offset<0 || offset>(long)length-8) throw new InvalidDataException("SID header extent");
            int size=8+4*Marshal.ReadByte(sid,1);
            if (size>length-offset) throw new InvalidDataException("SID extent");
            return new SecurityIdentifier(sid).Value;
        }
        static string Principal(NativeHandle t,int kind) { return WithToken(t,kind,(p,n)=> { if(n<IntPtr.Size) throw new InvalidDataException("Token SID pointer extent"); return SidAt(p,n,Marshal.ReadIntPtr(p)); }); }
        static bool PrivilegeEnabled(NativeHandle t,string name) {
            Win.Luid wanted; Win.Check(Win.LookupPrivilegeValueW(null,name,out wanted),"LookupPrivilegeValue");
            return WithToken(t,3,(p,n)=> {
                uint count=(uint)Marshal.ReadInt32(p); if (count>4096 || 4UL+12UL*count>n) throw new InvalidDataException("Privilege extent");
                for (int i=0;i<count;i++) {
                    int at=4+i*12;
                    if ((uint)Marshal.ReadInt32(p,at)==wanted.Low && Marshal.ReadInt32(p,at+4)==wanted.High)
                        return ((uint)Marshal.ReadInt32(p,at+8)&2)!=0;
                }
                return false;
            });
        }
        static void NoImpersonation() {
            IntPtr thread;
            if (Win.OpenThreadToken(Win.GetCurrentThread(),Win.TokenQuery,true,out thread)) {
                Win.CloseHandle(thread); throw new InvalidOperationException("Unexpected effective impersonation token");
            }
            if (Marshal.GetLastWin32Error()!=1008) throw new Win32Exception(Marshal.GetLastWin32Error(),"OpenThreadToken");
        }
        public Provisioner(string standard,string foreign) {
            if (IntPtr.Size!=8 || Environment.OSVersion.Platform!=PlatformID.Win32NT) throw new PlatformNotSupportedException("Native AMD64 Windows only");
            StandardSid=new SecurityIdentifier(standard).Value; ForeignSid=new SecurityIdentifier(foreign).Value;
            if (standard==foreign || !standard.StartsWith("S-1-5-21-",StringComparison.Ordinal) || !foreign.StartsWith("S-1-5-21-",StringComparison.Ordinal)) throw new InvalidDataException("Two distinct local SIDs required");
            NoImpersonation(); IntPtr raw;
            Win.Check(Win.OpenProcessToken(Win.GetCurrentProcess(),Win.TokenQuery|Win.TokenDuplicate,out raw),"OpenProcessToken");
            token=Own(raw,"provisioner-token");
            try {
                ActorSid=Principal(token,1);
                if (ActorSid==standard || ActorSid==foreign) throw new InvalidDataException("Provisioner cannot be test actor");
                Contract.RequireCapability(PrivilegeEnabled(token,"SeImpersonatePrivilege"),"pre-existing launcher SeImpersonatePrivilege");
                SystemDirectory=DirectoryFromSdk(true); WindowsDirectory=DirectoryFromSdk(false);
            } catch(Exception primary) {
                try { token.Close(); } catch(Exception cleanup) { throw new AggregateException(primary,cleanup); }
                throw;
            }
        }
        public static FileFact Inspect(NativeHandle file,bool requireSingleLink=true) {
            if (Win.GetFileType(file.Value)!=1) throw new InvalidDataException("Non-disk fixture object");
            IntPtr p=Marshal.AllocHGlobal(40);
            try {
                Win.Check(Win.GetFileInformationByHandleEx(file.Value,9,p,8),"FileAttributeTagInfo");
                if ((Marshal.ReadInt32(p)&0x400)!=0 || Marshal.ReadInt32(p,4)!=0) throw new InvalidDataException("Reparse fixture object");
                Win.Check(Win.GetFileInformationByHandleEx(file.Value,1,p,24),"FileStandardInfo");
                long length=Marshal.ReadInt64(p,8); bool directory=Marshal.ReadByte(p,21)!=0; int links=Marshal.ReadInt32(p,16);
                if (Marshal.ReadByte(p,20)!=0 || (!directory && requireSingleLink && links!=1)) throw new InvalidDataException("Delete-pending or hardlinked fixture object");
                Win.Check(Win.GetFileInformationByHandleEx(file.Value,18,p,24),"FileIdInfo");
                ulong volume=unchecked((ulong)Marshal.ReadInt64(p)); byte[] id=new byte[16]; Marshal.Copy(IntPtr.Add(p,8),id,0,16);
                IntPtr owner,dacl,sd; uint error=Win.GetSecurityInfo(file.Value,1,5,out owner,IntPtr.Zero,out dacl,IntPtr.Zero,out sd);
                if (error!=0) throw new Win32Exception((int)error,"GetSecurityInfo");
                try {
                    uint size=Win.GetSecurityDescriptorLength(sd); if (size<20 || size>65536) throw new InvalidDataException("Security descriptor extent");
                    byte[] bytes=new byte[size]; Marshal.Copy(sd,bytes,0,bytes.Length);
                    var descriptor=new RawSecurityDescriptor(bytes,0);
                    return new FileFact { volume=volume.ToString(System.Globalization.CultureInfo.InvariantCulture), file_id=Contract.Hex(id), directory=directory,length=length,links=links, descriptor=descriptor.GetSddlForm(AccessControlSections.Owner|AccessControlSections.Access) };
                } finally { Win.LocalFree(sd); }
            } finally { Marshal.FreeHGlobal(p); }
        }
        void Security(NativeHandle file,bool exact,bool systemObject=false) {
            var fact=Inspect(file,!systemObject); var sd=new RawSecurityDescriptor(fact.descriptor);
            string owner=sd.Owner.Value;
            if (exact ? owner!=ActorSid : owner!=ActorSid && owner!=Admin && owner!=SystemSid && owner!=Installer) throw new InvalidDataException("Untrusted fixture-chain owner");
            if (sd.DiscretionaryAcl==null || (exact && (sd.ControlFlags&ControlFlags.DiscretionaryAclProtected)==0)) throw new InvalidDataException("Unprotected/null fixture DACL");
            foreach (GenericAce ace in sd.DiscretionaryAcl) {
                var common=ace as CommonAce;
                if (common==null || common.IsCallback || (common.AceQualifier!=AceQualifier.AccessAllowed && common.AceQualifier!=AceQualifier.AccessDenied)) throw new InvalidDataException("Unknown fixture ACE");
                if (common.AceQualifier==AceQualifier.AccessDenied || (common.AceFlags&AceFlags.InheritOnly)!=0) continue;
                string sid=common.SecurityIdentifier.Value;
                if (sid==ActorSid || sid==Admin || sid==SystemSid || (systemObject && sid==Installer)) continue;
                uint mask=unchecked((uint)common.AccessMask);
                // Add-subdirectory alone cannot replace an already held ancestor.
                uint dangerous=exact || !fact.directory ? 0x500d0156u : 0x500d0152u;
                if ((mask&dangerous)!=0) throw new InvalidDataException("Outsider fixture-chain mutation ACE");
            }
        }
        static void Component(string name) {
            if (String.IsNullOrEmpty(name) || name=="." || name==".." || name.IndexOfAny(new[]{'\\','/',':','\0'})>=0 || name.EndsWith(".",StringComparison.Ordinal) || name.EndsWith(" ",StringComparison.Ordinal)) throw new InvalidDataException("Unsafe native component");
            string stem=name.Split('.')[0].ToUpperInvariant();
            if (new[]{"CON","PRN","AUX","NUL","CONIN$","CONOUT$"}.Contains(stem) || (stem.Length==4 && (stem.StartsWith("COM") || stem.StartsWith("LPT")) && stem[3]>='1' && stem[3]<='9')) throw new InvalidDataException("Reserved native component");
        }
        NativeHandle Relative(NativeHandle parent,string name,bool directory,uint access,uint share,string sddl) {
            Component(name); byte[] encoded=Encoding.Unicode.GetBytes(name); if (encoded.Length>65534) throw new InvalidDataException("Native component length");
            IntPtr chars=Marshal.StringToHGlobalUni(name), unicode=IntPtr.Zero, security=IntPtr.Zero;
            try {
                var text=new Win.Unicode { Length=(ushort)encoded.Length, MaximumLength=(ushort)encoded.Length, Buffer=chars };
                unicode=Marshal.AllocHGlobal(Marshal.SizeOf<Win.Unicode>()); Marshal.StructureToPtr(text,unicode,false);
                if (sddl!=null) Win.Check(Win.ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl,1,out security,IntPtr.Zero),"Convert SDDL");
                var attr=new Win.ObjectAttributes { Length=(uint)Marshal.SizeOf<Win.ObjectAttributes>(),Root=parent.Value,Name=unicode,Attributes=0x1040,Security=security };
                IntPtr raw; Win.IoStatus io=default;
                int status=Win.NtCreateFile(out raw,access,ref attr,ref io,IntPtr.Zero,0x80,share,sddl==null ? 1u:2u,0x20u|0x200000u|(directory ? 1u:0x40u),IntPtr.Zero,0);
                if (status==0x103 || io.Status==new IntPtr(0x103)) Environment.FailFast("Unexpected pending synchronous fixture open");
                if (status!=0) throw new InvalidOperationException("NtCreateFile refused: 0x"+status.ToString("x8"));
                var result=Own(raw,"file:"+name);
                if (io.Status!=IntPtr.Zero) throw new InvalidOperationException("Native final status failure");
                var fact=Inspect(result,false); if (fact.directory!=directory) throw new InvalidDataException("Fixture object type mismatch"); return result;
            } finally { if (security!=IntPtr.Zero) Win.LocalFree(security); if (unicode!=IntPtr.Zero) Marshal.FreeHGlobal(unicode); Marshal.FreeHGlobal(chars); }
        }
        public HeldTree Retain(string path) { return RetainPath(path,false); }
        HeldTree RetainPath(string path,bool systemChain) {
            string full=Path.GetFullPath(path);
            if (full.Length<3 || full[1]!=':' || full[2]!='\\' || path!=full) throw new InvalidDataException("Canonical absolute local drive path required");
            string root=full.Substring(0,3);
            if (Win.GetDriveTypeW(root)!=3) throw new InvalidDataException("Fixed local drive required");
            var chain=new HeldTree { Path=full };
            var first=Own(Win.CreateFileW(root,Win.ReadAttr|Win.Traverse|Win.ReadControl|Win.Sync,3,IntPtr.Zero,3,0x02200000,IntPtr.Zero),"volume-root");
            var fs=new StringBuilder(32); uint serial,max,flags;
            Win.Check(Win.GetVolumeInformationByHandleW(first.Value,IntPtr.Zero,0,out serial,out max,out flags,fs,32),"NTFS query");
            if (fs.ToString()!="NTFS") throw new InvalidDataException("NTFS required");
            Security(first,false,systemChain); chain.Handles.Add(first); chain.Keys.Add(Inspect(first).Key); chain.Names.Add(root);
            foreach (string name in full.Substring(3).Split(new[]{'\\'},StringSplitOptions.RemoveEmptyEntries)) {
                var next=Relative(chain.Last,name,true,Win.ReadAttr|Win.Traverse|Win.ReadControl|Win.Sync,3,null);
                Security(next,false,systemChain); chain.Handles.Add(next); chain.Keys.Add(Inspect(next).Key); chain.Names.Add(name);
            }
            chains.Add(chain); return chain;
        }
        string Descriptor(bool standardRead) {
            return "O:"+ActorSid+"D:P(A;OICI;FA;;;"+ActorSid+")(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)"+(standardRead ? "(A;OICI;0x1200a9;;;"+StandardSid+")" : "");
        }
        public HeldTree CreateTree(HeldTree parent,string name,bool standardRead) {
            Verify(parent); var file=Relative(parent.Last,name,true,0x12019fu|Win.Traverse,3,Descriptor(standardRead)); Security(file,true);
            var tree=new HeldTree { Path=Path.Combine(parent.Path,name) };
            tree.Handles.AddRange(parent.Handles); tree.Keys.AddRange(parent.Keys); tree.Names.AddRange(parent.Names);
            tree.Handles.Add(file); tree.Keys.Add(Inspect(file).Key); tree.Names.Add(name); chains.Add(tree);
            resources.Add(new { kind="owned-tree",path=tree.Path,identity=Inspect(file).Key,created=true }); Point("acquire:tree:"+name); return tree;
        }
        public void Verify(HeldTree tree) {
            for(int i=0;i<tree.Handles.Count;i++) {
                Security(tree.Handles[i],false);
                if (Inspect(tree.Handles[i]).Key!=tree.Keys[i]) throw new InvalidDataException("Held chain identity changed");
                if(i>0) {
                    var named=Relative(tree.Handles[i-1],tree.Names[i],true,Win.ReadAttr|Win.Traverse|Win.ReadControl|Win.Sync,3,null);
                    try { if (Inspect(named).Key!=tree.Keys[i]) throw new InvalidDataException("Named chain identity changed"); } finally { named.Close(); }
                }
            }
        }
        byte[] Read(NativeHandle file,int limit) {
            using (var safe=new Microsoft.Win32.SafeHandles.SafeFileHandle(file.Value,false))
            using (var stream=new FileStream(safe,FileAccess.Read)) {
                if(stream.Length>limit) throw new InvalidDataException("File size bound"); stream.Position=0;
                byte[] bytes=new byte[checked((int)stream.Length)]; int position=0;
                while(position<bytes.Length) { int got=stream.Read(bytes,position,bytes.Length-position); if(got==0) throw new EndOfStreamException(); position+=got; }
                if(stream.ReadByte()!=-1) throw new InvalidDataException("File grew"); return bytes;
            }
        }
        void Write(NativeHandle file,byte[] bytes) {
            using(var safe=new Microsoft.Win32.SafeHandles.SafeFileHandle(file.Value,false))
            using(var stream=new FileStream(safe,FileAccess.Write)) { stream.Write(bytes,0,bytes.Length); stream.Flush(true); }
        }
        NativeHandle NewFile(HeldTree tree,string name,byte[] bytes,bool standardRead=false) {
            Verify(tree); var file=Relative(tree.Last,name,false,0x12019fu,1,Descriptor(standardRead)); Security(file,true); Write(file,bytes); return file;
        }
        public string PrepareCandidate(HeldTree tree,string source,string revision,string lockHash,string classificationJson) {
            Contract.RequireHex(revision,40); Contract.RequireHex(lockHash,64); launch=tree; Verify(tree);
            using(var classification=Contract.Json(Encoding.UTF8.GetBytes(classificationJson),"schema_version","system_imports","system_modules","api_contract_hosts","private_modules")) {
                var spec=classification.RootElement;
                if(spec.GetProperty("schema_version").GetInt32()!=1) throw new InvalidDataException("Dependency schema");
                // This slice intentionally has no private dependencies. A candidate needing one fails setup until separately frozen/reviewed.
                if(spec.GetProperty("private_modules").GetArrayLength()!=0) throw new InvalidDataException("Private dependency inventory not implemented; setup refused");
                var imports=new HashSet<string>(spec.GetProperty("system_imports").EnumerateArray().Select(x=>x.GetString()),StringComparer.Ordinal);
                foreach(var entry in spec.GetProperty("system_modules").EnumerateArray()) {
                    string name=entry.GetString(); Component(name);
                    if(name!=name.ToLowerInvariant() || !name.EndsWith(".dll",StringComparison.Ordinal)) throw new InvalidDataException("System module classification"); allowedSystemModules.Add(name);
                }
                var sourceParent=Retain(Path.GetDirectoryName(source));
                var original=Relative(sourceParent.Last,Path.GetFileName(source),false,0x120089,1,null); Security(original,false);
                byte[] bytes=Read(original,512*1024*1024); string[] actual=PeImports.Read(bytes);
                var hosts=spec.GetProperty("api_contract_hosts").EnumerateObject().ToDictionary(x=>x.Name,x=>x.Value.GetString(),StringComparer.Ordinal);
                foreach(string name in actual) Contract.RequireImport(name,imports,allowedSystemModules,hosts);
                var systemTree=RetainPath(SystemDirectory,true);
                foreach(string module in allowedSystemModules.OrderBy(x=>x,StringComparer.Ordinal)) {
                    string path=Path.Combine(SystemDirectory,module);
                    if(!File.Exists(path)) {
                        if(actual.Contains(module,StringComparer.Ordinal) || (module=="ucrtbase.dll" && actual.Any(x=>x.StartsWith("api-",StringComparison.Ordinal)))) throw new InvalidDataException("Required system module missing: "+module);
                        continue; // An unused allowlisted module is not a required dependency.
                    }
                    var systemFile=Relative(systemTree.Last,module,false,0x120089,1,null);Security(systemFile,false,true);
                    string id=Inspect(systemFile,false).Key;string hash=Contract.Hash(Read(systemFile,512*1024*1024));
                    systemModuleFacts.Add(path,id+"|"+hash);resources.Add(new {kind="prelaunch-system-module",path,identity=id,hash});
                }
                // Imports may be a subset: optimization can remove an approved dependency, but cannot add one.
                candidateHash=Contract.Hash(bytes); executable=Path.Combine(tree.Path,"surge-persistence-probe.exe");
                var copy=NewFile(tree,"surge-persistence-probe.exe",bytes,true); copy.Close();
                var immutable=Relative(tree.Last,"surge-persistence-probe.exe",false,0x120089,1,null); Security(immutable,true);
                Contract.RequireBinding(candidateHash,Contract.Hash(Read(immutable,512*1024*1024)),"Candidate copy hash");
                protectedModules.Add(executable,Inspect(immutable).Key);
                var manifest=new { schema_version=1,revision,cargo_lock_hash=lockHash,candidate_hash=candidateHash,source,source_identity=Inspect(original).Key,copy_identity=Inspect(immutable).Key,imports=actual,classification_hash=Contract.Hash(Encoding.UTF8.GetBytes(classificationJson)),os_build=Environment.OSVersion.Version.ToString() };
                string json=JsonSerializer.Serialize(manifest); NewFile(tree,"candidate-manifest.json",Encoding.UTF8.GetBytes(json)); return json;
            }
        }
        public NativeHandle DuplicateFor(string filter) {
            if(!Contract.Elevated(filter)) throw new InvalidDataException("Standard actor must use standard credentials");
            NoImpersonation(); uint rights=Win.TokenQuery|Win.TokenDuplicate|Win.TokenAssign;
            bool admin=filter==Contract.Filters[0]; rights|=admin ? Win.TokenDefault:Win.TokenPrivileges;
            IntPtr raw; Win.Check(Win.DuplicateTokenEx(token.Value,rights,IntPtr.Zero,2,1,out raw),"DuplicateTokenEx");
            var duplicate=Own(raw,admin ? "admin-owner-token":"restore-token");
            if(admin) {
                bool eligible=WithToken(duplicate,2,(p,n)=> {
                    uint count=(uint)Marshal.ReadInt32(p); int offset=IntPtr.Size;
                    if(count>4096 || (ulong)offset+16UL*count>n) throw new InvalidDataException("TokenGroups extent");
                    for(int i=0;i<count;i++) {
                        IntPtr sid=Marshal.ReadIntPtr(p,offset+i*16); uint flags=(uint)Marshal.ReadInt32(p,offset+i*16+8);
                        if(SidAt(p,n,sid)==Admin) return (flags&8)!=0 && (flags&0x10)==0;
                    }
                    return false;
                });
                Contract.RequireCapability(eligible,"Administrators default-owner eligibility");
                var sid=new SecurityIdentifier(Admin); byte[] bytes=new byte[sid.BinaryLength]; sid.GetBinaryForm(bytes,0);
                IntPtr storage=Marshal.AllocHGlobal(bytes.Length), owner=Marshal.AllocHGlobal(IntPtr.Size);
                try {
                    Marshal.Copy(bytes,0,storage,bytes.Length); Marshal.WriteIntPtr(owner,storage);
                    Win.Check(Win.SetTokenInformation(duplicate.Value,4,owner,(uint)IntPtr.Size),"Set duplicate TokenOwner");
                } finally { Marshal.FreeHGlobal(owner); Marshal.FreeHGlobal(storage); }
                if(Principal(duplicate,4)!=Admin || Principal(duplicate,1)==Admin || Principal(duplicate,1)!=ActorSid) throw new InvalidDataException("Actual duplicate owner/user mismatch");
            } else {
                Win.Luid id; Win.Check(Win.LookupPrivilegeValueW(null,"SeRestorePrivilege",out id),"Lookup SeRestorePrivilege");
                var value=new Win.Privilege { Count=1, Id=id, Attributes=2 };
                Win.Check(Win.AdjustTokenPrivileges(duplicate.Value,false,ref value,0,IntPtr.Zero,IntPtr.Zero),"Adjust duplicate SeRestorePrivilege");
                int error=Marshal.GetLastWin32Error(); if(error!=0) throw new Win32Exception(error,"SeRestorePrivilege not assigned");
                Contract.RequireCapability(PrivilegeEnabled(duplicate,"SeRestorePrivilege"),"duplicate SeRestorePrivilege");
            }
            return duplicate;
        }
        NativeHandle Event(string name) { return Own(Win.CreateEventW(IntPtr.Zero,true,false,null),name); }
        string Transfer(NativeHandle source,Child child,uint rights) {
            IntPtr result; Win.Check(Win.DuplicateHandle(Win.GetCurrentProcess(),source.Value,child.Process.Value,out result,rights,false,0),"Duplicate child handle");
            resources.Add(new { kind="child-handle", child=child.Pid,rights,inherit=false }); Point("acquire:child-handle");
            return unchecked((ulong)result.ToInt64()).ToString(System.Globalization.CultureInfo.InvariantCulture);
        }
        Dictionary<string,string> BaseEnvironment() {
            return new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase) {
                {"SystemRoot",WindowsDirectory},{"WINDIR",WindowsDirectory}, {"PATH",SystemDirectory+";"+launch.Path},
                {"TEMP",launch.Path},{"TMP",launch.Path},{"USERPROFILE",launch.Path},
                {"HOMEDRIVE",Path.GetPathRoot(launch.Path).TrimEnd('\\')},{"HOMEPATH",launch.Path.Substring(2)},
                {"RUST_TEST_THREADS","1"}
            };
        }
        public Dictionary<string,string> ActorEnvironment(string filter,string nonce,string data,string standardData,string receipt,string ready) {
            Contract.Elevated(filter); Contract.RequireHex(nonce,32);
            if(candidateHash==null) throw new InvalidOperationException("Candidate not frozen");
            var env=BaseEnvironment(); bool elevated=Contract.Elevated(filter);
            env.Add("SURGE_FIXTURE_HASH",candidateHash);env.Add("SURGE_FIXTURE_FILTER",filter);env.Add("SURGE_FIXTURE_NONCE",nonce);
            env.Add("SURGE_FIXTURE_ACTOR_SID",elevated ? ActorSid:StandardSid);env.Add("SURGE_FIXTURE_STANDARD_SID",StandardSid);env.Add("SURGE_FIXTURE_FOREIGN_SID",ForeignSid);
            env.Add("SURGE_FIXTURE_DATA",data);env.Add("SURGE_FIXTURE_STANDARD_DATA",standardData);env.Add("SURGE_FIXTURE_RECEIPT",receipt);env.Add("SURGE_FIXTURE_READY_RECEIPT",ready);
            if(!elevated) env.Add("SURGE_NATIVE_PROBE_SID",StandardSid);
            return env;
        }
        public Child LaunchElevated(string filter,string nonce,HeldTree data,HeldTree standardData,HeldTree invocation) {
            if(!Contract.Elevated(filter)) throw new InvalidDataException("Wrong actor role");
            Verify(launch);Verify(data);Verify(standardData);Verify(invocation);
            var duplicate=DuplicateFor(filter);
            string receipt=Path.Combine(invocation.Path,"complete.json"), ready=Path.Combine(invocation.Path,"ready.json"), bootstrap=Path.Combine(invocation.Path,"bootstrap.json");
            var env=ActorEnvironment(filter,nonce,data.Path,standardData.Path,receipt,ready);env.Add("SURGE_FIXTURE_BOOTSTRAP",bootstrap);
            byte[] block=Encoding.Unicode.GetBytes(Contract.EnvironmentBlock(env)); IntPtr environment=Marshal.AllocHGlobal(block.Length);
            var child=new Child { Filter=filter,Actor=ActorSid,Nonce=nonce,Receipt=receipt,ReadyReceipt=ready };
            children.Add(child); // Slot exists even if later resource registration throws.
            try {
                Marshal.Copy(block,0,environment,block.Length);
                var startup=new Win.Startup { cb=(uint)Marshal.SizeOf<Win.Startup>() };Win.ProcessInfo info;
                Win.Check(Win.CreateProcessWithTokenW(duplicate.Value,0,executable,new StringBuilder(Contract.Command(executable,filter)),0x404,environment,launch.Path,ref startup,out info),"CreateProcessWithTokenW");
                // Register BOTH original handles before injected failure or any inspection.
                child.Process=new NativeHandle(info.Process,"child-process"); handles.Add(child.Process);
                child.Thread=new NativeHandle(info.Thread,"child-thread"); handles.Add(child.Thread);child.Pid=info.Pid;
                resources.Add(new { kind="process",pid=info.Pid,filter,launched=true });Point("acquire:process");
                long exit,kernel,user;Win.Check(Win.GetProcessTimes(child.Process.Value,out long creation,out exit,out kernel,out user),"GetProcessTimes");child.CreationTime=creation;
                child.Ready=Event("ready-event");child.Proceed=Event("proceed-event");child.Cancel=Event("cancel-event");
                var stdout=NewFile(invocation,"stdout.log",Array.Empty<byte>());var stderr=NewFile(invocation,"stderr.log",Array.Empty<byte>());child.Files.Add(stdout);child.Files.Add(stderr);
                var outFact=Inspect(stdout);var errFact=Inspect(stderr);
                var payload=new {
                    schema_version=1,candidate_hash=candidateHash,filter,nonce,actor_sid=ActorSid,standard_sid=StandardSid,foreign_sid=ForeignSid,pid=child.Pid,
                    stdout=new { handle=Transfer(stdout,child,0x120082),volume=outFact.volume,file_id=outFact.file_id },
                    stderr=new { handle=Transfer(stderr,child,0x120082),volume=errFact.volume,file_id=errFact.file_id },
                    ready_handle=Transfer(child.Ready,child,2),proceed_handle=Transfer(child.Proceed,child,Win.Sync),cancel_handle=Transfer(child.Cancel,child,Win.Sync)
                };
                NewFile(invocation,"bootstrap.json",JsonSerializer.SerializeToUtf8Bytes(payload));
                if(Win.ResumeThread(child.Thread.Value)==UInt32.MaxValue) throw new Win32Exception(Marshal.GetLastWin32Error(),"ResumeThread");
                child.Thread.Close();
                return child;
            } finally { Marshal.FreeHGlobal(environment); }
        }
        public Child RegisterStandard(IntPtr borrowedProcess,uint pid,string filter,string nonce,string receipt) {
            if(Contract.Elevated(filter)) throw new InvalidDataException("Wrong standard actor role");
            IntPtr copy;Win.Check(Win.DuplicateHandle(Win.GetCurrentProcess(),borrowedProcess,Win.GetCurrentProcess(),out copy,0x100451,false,0),"Retain standard process");
            var child=new Child { Process=new NativeHandle(copy,"standard-process"),Pid=pid,Filter=filter,Actor=StandardSid,Nonce=nonce,Receipt=receipt };
            handles.Add(child.Process);children.Add(child);resources.Add(new {kind="process",pid,filter,launched=true});Point("acquire:standard-process");
            long exit,kernel,user;Win.Check(Win.GetProcessTimes(child.Process.Value,out long creation,out exit,out kernel,out user),"GetProcessTimes");child.CreationTime=creation;
            return child;
        }
        public void ObserveReady(Child child) {
            uint result=Win.WaitForSingleObject(child.Ready.Value,30000);
            if(result!=0) { child.TimedOut=result==258; child.Settlement.AcceptanceFailed(); throw new InvalidOperationException("Actor readiness failed, wait="+result); }
            ValidateReceipt(child,true);
            IntPtr raw;Win.Check(Win.OpenProcessToken(child.Process.Value,Win.TokenQuery,out raw),"Query actual child token");
            var actual=Own(raw,"actual-child-token");
            try {
                if(Principal(actual,1)!=ActorSid) throw new InvalidDataException("Actual child actor differs");
                if(child.Filter==Contract.Filters[0] && Principal(actual,4)!=Admin) throw new InvalidDataException("Child default owner differs");
                if(child.Filter==Contract.Filters[2] && !PrivilegeEnabled(actual,"SeRestorePrivilege")) throw new InvalidDataException("Child restore privilege absent");
            } finally { actual.Close(); }
            var image=new StringBuilder(32768);uint length=32768;
            Win.Check(Win.QueryFullProcessImageNameW(child.Process.Value,0,image,ref length),"Query child image");
            if(!String.Equals(image.ToString(),executable,StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("Child image path differs");
            AuditModules(child);Verify(launch);Win.Check(Win.SetEvent(child.Proceed.Value),"Release verified actor");
        }
        void AuditModules(Child child) {
            IntPtr[] modules=new IntPtr[1024];uint bytes;
            Win.Check(Win.EnumProcessModulesEx(child.Process.Value,modules,(uint)(modules.Length*IntPtr.Size),out bytes,3),"Enumerate child modules");
            if(bytes==0 || bytes>modules.Length*IntPtr.Size || bytes%IntPtr.Size!=0) throw new InvalidDataException("Module enumeration extent");
            for(int i=0;i<bytes/IntPtr.Size;i++) {
                var value=new StringBuilder(32768);uint size=Win.GetModuleFileNameExW(child.Process.Value,modules[i],value,32768);
                if(size==0 || size>=32768) throw new InvalidDataException("Module name query");
                string path=value.ToString(), name=Path.GetFileName(path).ToLowerInvariant();
                bool system=String.Equals(Path.GetDirectoryName(path),SystemDirectory,StringComparison.OrdinalIgnoreCase);
                var parent=system ? RetainPath(SystemDirectory,true):Retain(Path.GetDirectoryName(path));
                var file=Relative(parent.Last,Path.GetFileName(path),false,0x120089,1,null);Security(file,false,system);
                if(protectedModules.TryGetValue(path,out string identity)) {
                    if(Inspect(file).Key!=identity || Contract.Hash(Read(file,512*1024*1024))!=candidateHash) throw new InvalidDataException("Loaded candidate identity mismatch");
                } else {
                    if(!String.Equals(Path.GetDirectoryName(path),SystemDirectory,StringComparison.OrdinalIgnoreCase) || !allowedSystemModules.Contains(name)) throw new InvalidDataException("Unreviewed loaded system module: "+name);
                    string actual=Inspect(file,false).Key+"|"+Contract.Hash(Read(file,512*1024*1024));
                    if(!systemModuleFacts.TryGetValue(path,out string expected) || actual!=expected) throw new InvalidDataException("Loaded system module differs from prelaunch inventory");
                }
            }
        }
        public string ValidateReceipt(Child child,bool ready) {
            string receiptPath=ready ? child.ReadyReceipt:child.Receipt;
            var receiptFile=Own(Win.CreateFileW(receiptPath,0x120089,1,IntPtr.Zero,3,0x200000,IntPtr.Zero),"receipt");
            byte[] bytes;
            try { if(Inspect(receiptFile).directory) throw new InvalidDataException("Receipt is directory"); bytes=Read(receiptFile,65536); }
            finally { receiptFile.Close(); }
            string[] fields=ready ? new[]{"schema_version","phase","candidate_hash","filter","nonce","actor_sid","pid"} : new[]{"schema_version","phase","candidate_hash","filter","nonce","actor_sid","pid","observation","facts"};
            using(var doc=Contract.Json(bytes,fields)) {
                var value=doc.RootElement;
                if(value.GetProperty("schema_version").GetInt32()!=1 || value.GetProperty("phase").GetString()!=(ready ? "ready":"complete") || value.GetProperty("candidate_hash").GetString()!=candidateHash || value.GetProperty("filter").GetString()!=child.Filter || value.GetProperty("nonce").GetString()!=child.Nonce || value.GetProperty("actor_sid").GetString()!=child.Actor || value.GetProperty("pid").GetUInt32()!=child.Pid) throw new InvalidDataException("Receipt binding differs");
                if(!ready) {
                    string[] observations={"stage1 actual administrative TokenOwner production creation=PASS","stage1 standard-user creation below administrative ancestor=PASS","stage1 foreign-owner fixture exact user ACL setup=PASS","stage1 foreign-owner exact user ACL unchanged security refusal=PASS"};
                    if(value.GetProperty("observation").GetString()!=observations[Array.IndexOf(Contract.Filters,child.Filter)] || value.GetProperty("facts").ValueKind!=JsonValueKind.Object || !value.GetProperty("facts").EnumerateObject().Any()) throw new InvalidDataException("Missing fixture observation facts");
                    Contract.RequireCleanup(child.Filter,value.GetProperty("facts"));
                }
            }
            return Encoding.UTF8.GetString(bytes);
        }
        public void WaitSuccessful(Child child) {
            uint result=Win.WaitForSingleObject(child.Process.Value,120000);
            if(result!=0) { child.TimedOut=result==258; child.Settlement.AcceptanceFailed(); throw new InvalidOperationException("Actor completion failed, wait="+result); }
            RecordExit(child);if(child.ExitCode!=0) throw new InvalidOperationException("Actor failed: exit="+child.ExitCode);
            ValidateReceipt(child,false);
        }
        void RecordExit(Child child) {
            Win.Check(Win.GetExitCodeProcess(child.Process.Value,out uint code),"GetExitCodeProcess");child.ExitCode=code;child.Settlement.ObserveExit();
        }
        public void RecordFailure(string phase,Exception error) { Errors.Add(phase+": "+error.GetType().Name+": "+error.Message); }
        void Attempt(string phase,Action action) { try { action();Point("cleanup:"+phase); } catch(Exception error) { RecordFailure(phase,error); } }
        public void SettleBounded() {
            foreach(var child in children) {
                if(child.Cancel!=null && child.Cancel.Value!=IntPtr.Zero) Attempt("cancel:"+child.Pid,()=>Win.Check(Win.SetEvent(child.Cancel.Value),"Signal cancel"));
            }
            foreach(var child in children) {
                if(child.Process==null || child.ExitObserved) continue;
                try {
                    uint result=Win.WaitForSingleObject(child.Process.Value,0);
                    if(result!=0) {
                        if(result!=258) throw new Win32Exception(Marshal.GetLastWin32Error(),"Initial settlement wait");
                        Win.Check(Win.TerminateProcess(child.Process.Value,0xe0000001),"Terminate owned actor");
                        result=Win.WaitForSingleObject(child.Process.Value,10000);
                    }
                    if(result!=0) throw new InvalidOperationException("No observed actor exit, wait="+result);
                    RecordExit(child);
                } catch(Exception error) { PreserveResources=true;child.Settlement.SettlementFailed();RecordFailure("settle:"+child.Pid,error); }
            }
        }
        public bool HasUnsettled { get { return children.Any(c=>c.Process!=null && !c.ExitObserved); } }
        public void HoldUntilExit(Action checkpoint) {
            if(!HasUnsettled) return;
            PreserveResources=true; Attempt("checkpoint",checkpoint);
            // This is a sticky failed-owner hold, never an acceptance retry or new deadline.
            var reported=new HashSet<uint>();
            while(HasUnsettled) {
                bool observationFailed=false;
                foreach(var child in children.Where(c=>c.Process!=null && !c.ExitObserved)) {
                    try {
                        uint result=Win.WaitForSingleObject(child.Process.Value,1000);
                        if(result==0) RecordExit(child);
                        else if(result!=258) throw new Win32Exception(Marshal.GetLastWin32Error(),"Retained child wait failed");
                    } catch(Exception error) {
                        observationFailed=true;
                        if(reported.Add(child.Pid)) { RecordFailure("hold-observation:"+child.Pid,error);Attempt("checkpoint",checkpoint); }
                    }
                }
                // One private managed wait gate; no allocation/injection after a
                // broken observation, no repeated failure growth, no acceptance retry.
                if(observationFailed) lock(failedObservationGate) System.Threading.Monitor.Wait(failedObservationGate,1000);
            }
            Attempt("checkpoint",checkpoint);
        }
        public void CloseSettledHandles() {
            if(HasUnsettled) throw new InvalidOperationException("Cannot close unsettled owners");
            foreach(var child in children.Where(c=>c.Process!=null)) child.Settlement.RequireClosable();
            CleanupBatch.Run(handles.AsEnumerable().Reverse().Where(h=>h.Value!=IntPtr.Zero)
                .Select(h=>new KeyValuePair<string,Action>("close:"+h.Kind,()=> { h.Close();Point("cleanup:close:"+h.Kind); })),RecordFailure);
        }
        public string CreateOrdinaryTree(HeldTree parent,string name) {
            Verify(parent);string sd="O:"+ActorSid+"D:P(A;OICI;FA;;;"+ActorSid+")(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;FA;;;"+StandardSid+")";
            var file=Relative(parent.Last,name,true,0x12019f|Win.Traverse,3,sd);
            string path=Path.Combine(parent.Path,name);resources.Add(new {kind="ordinary-tree",path,identity=Inspect(file).Key,created=true});Point("acquire:ordinary-tree");
            return path; // Retain the original no-delete-sharing parent through all actor exits.
        }
        public object PinToolArtifact(string path) {
            // Path comes only from the provisioner's already-loaded runtime/SDK assembly
            // inventory, never a probe receipt. TrustedInstaller may service OS/toolchain
            // ancestry; exact private launch directories still forbid its mutation ACEs.
            var parent=RetainPath(Path.GetDirectoryName(path),true);
            var file=Relative(parent.Last,Path.GetFileName(path),false,0x120089,1,null);Security(file,false,true);
            return new {path,identity=Inspect(file,false).Key,hash=Contract.Hash(Read(file,512*1024*1024))};
        }
        public string TreeIdentity(HeldTree tree) { return Inspect(tree.Last).Key; }
        public string ObjectIdentity(string path) {
            var parent=Retain(Path.GetDirectoryName(path));
            var file=Relative(parent.Last,Path.GetFileName(path),true,Win.ReadAttr|Win.Traverse|Win.ReadControl|Win.Sync,3,null);
            try { return Inspect(file).Key; } finally { file.Close(); }
        }
        static Dictionary<string,bool> CleanupInventory(string role) {
            var names=new Dictionary<string,bool>(StringComparer.OrdinalIgnoreCase) {{"",true}};
            if(role=="bootstrap") {
                foreach(string file in new[]{"windows-native-provisioner.cs","windows-native-dependencies.json","Surge.NativeFixtures.dll"}) names.Add(file,false);
            } else if(role=="ordinary") {
                foreach(string actor in new[]{"standard-admin","standard-foreign"})
                    foreach(string suffix in new[]{"-complete.json","-stdout.log","-stderr.log"}) names.Add(actor+suffix,false);
            } else if(role=="launch") {
                names.Add("surge-persistence-probe.exe",false);names.Add("candidate-manifest.json",false);
                foreach(string directory in new[]{"admin-data","admin-data/admin-production","standard-data","standard-data/standard-production","standard-data/foreign-production","admin-owner-invocation","foreign-setup-invocation"}) names.Add(directory,true);
                foreach(string invocation in new[]{"admin-owner-invocation","foreign-setup-invocation"})
                    foreach(string file in new[]{"bootstrap.json","ready.json","complete.json","stdout.log","stderr.log"}) names.Add(invocation+"/"+file,false);
            } else throw new InvalidDataException("Unknown cleanup role");
            return names;
        }
        public void RemoveOwnedTree(string path,string expectedIdentity,string role) {
            if(HasUnsettled || PreserveResources) throw new InvalidOperationException("Cleanup prohibited for uncertain ownership");
            var inventory=CleanupInventory(role);var opened=new List<NativeHandle>();
            var parent=Retain(Path.GetDirectoryName(path));
            try {
                CaptureCleanup(parent.Last,Path.GetFileName(path),path,"",expectedIdentity,inventory,opened);
                // Entire finite inventory passes before any deletion. Each target stays
                // open without delete sharing and disposition uses that same SDK handle.
                foreach(var file in opened.AsEnumerable().Reverse()) {
                    uint disposition=1;Win.Check(Win.SetFileInformationByHandle(file.Value,21,ref disposition,4),"Remove original owned object");file.Close();
                }
            } finally {
                foreach(var file in opened.AsEnumerable().Reverse()) if(file.Value!=IntPtr.Zero) Attempt("cleanup-target-close",file.Close);
                foreach(var file in parent.Handles.AsEnumerable().Reverse()) if(file.Value!=IntPtr.Zero) Attempt("cleanup-parent-close",file.Close);
            }
        }
        void CaptureCleanup(NativeHandle parent,string name,string path,string relative,string expected,Dictionary<string,bool> inventory,List<NativeHandle> opened) {
            if(!inventory.TryGetValue(relative,out bool directory)) throw new InvalidDataException("Unexpected object preserved during cleanup: "+relative);
            var file=Relative(parent,name,directory,0x120089u|0x10000u|(directory ? Win.Traverse:0u),3,null);opened.Add(file);
            var fact=Inspect(file);
            if(fact.directory!=directory || (expected!=null && fact.Key!=expected)) throw new InvalidDataException("Owned cleanup identity/type changed");
            if(!directory) return;
            string[] entries=Directory.GetFileSystemEntries(path);
            if(entries.Length>inventory.Count) throw new InvalidDataException("Unexpected cleanup inventory size");
            foreach(string entry in entries) {
                string child=Path.GetFileName(entry);Component(child);
                CaptureCleanup(file,child,entry,relative.Length==0 ? child:relative+"/"+child,null,inventory,opened);
            }
        }
        public string SnapshotParentToken() {
            NoImpersonation();return Principal(token,1)+"|"+Principal(token,4)+"|"+Contract.Hash(TokenBytes(token,3));
        }
    }
}
