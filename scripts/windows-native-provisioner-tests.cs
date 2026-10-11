// Pure managed contract/failure tests. These do NOT establish Windows native acceptance.
using System;
using System.IO;
using System.Text;
using System.Collections.Generic;
using Surge.NativeFixtures;

public static class ProvisionerContractTests {
    static int checks;
    static void Equal<T>(T expected,T actual,string name) {
        checks++;if(!EqualityComparer<T>.Default.Equals(expected,actual)) throw new Exception(name+": mismatch");
    }
    static void Refuses(Action action,string name) {
        checks++;try { action(); } catch(Exception e) when(e is InvalidDataException || e is InvalidOperationException || e is OverflowException) { return; }
        throw new Exception(name+": unsafe input accepted");
    }
    static void U16(byte[] b,int p,ushort n) { Array.Copy(BitConverter.GetBytes(n),0,b,p,2); }
    static void U32(byte[] b,int p,uint n) { Array.Copy(BitConverter.GetBytes(n),0,b,p,4); }
    static byte[] Image() {
        // One AMD64 .idata section, one ordinary import and a zero terminator.
        var b=new byte[1024];U16(b,0,0x5a4d);U32(b,0x3c,0x80);U32(b,0x80,0x4550);
        U16(b,0x84,0x8664);U16(b,0x86,1);U16(b,0x94,240);U16(b,0x98,0x20b);
        U32(b,0x98+108,16);U32(b,0x98+120,0x1000);U32(b,0x98+124,40);
        int section=0x98+240;U32(b,section+8,512);U32(b,section+12,0x1000);U32(b,section+16,512);U32(b,section+20,512);
        U32(b,512+12,0x1060);Encoding.ASCII.GetBytes("kernel32.dll\0").CopyTo(b,608);return b;
    }
    public static int Main() {
        Equal("\"\"",Contract.Quote(""),"empty argv");
        Equal("\"C:\\a b\\\\\"",Contract.Quote("C:\\a b\\"),"trailing slash argv");
        Equal("\"a\\\"b\"",Contract.Quote("a\"b"),"embedded quote argv");
        Equal("\"資料\"",Contract.Quote("資料"),"unicode argv");
        Refuses(()=>Contract.Quote("bad\0value"),"NUL argv");
        Refuses(()=>Contract.Command("x.exe","--list"),"filter injection");
        Refuses(()=>Contract.Command(new string('a',1100),Contract.Filters[0]),"command length");
        Equal("A=1\0z=2\0\0",Contract.EnvironmentBlock(new Dictionary<string,string>{{"z","2"},{"A","1"}}),"sorted Unicode environment");
        Refuses(()=>Contract.EnvironmentBlock(new Dictionary<string,string>{{"A","1"},{"a","2"}}),"environment case collision");
        Refuses(()=>Contract.EnvironmentBlock(new Dictionary<string,string>{{"A=B","1"}}),"environment injection");
        Refuses(()=>Contract.EnvironmentBlock(new Dictionary<string,string>{{"A","x\0B=y"}}),"environment value injection");
        using(var doc=Contract.Json(Encoding.UTF8.GetBytes("{\"a\":1}"),"a")) Equal(1,doc.RootElement.GetProperty("a").GetInt32(),"strict JSON positive");
        Refuses(()=>Contract.Json(Encoding.UTF8.GetBytes("{\"a\":1,\"a\":2}"),"a"),"duplicate JSON");
        Refuses(()=>Contract.Json(Encoding.UTF8.GetBytes("{\"a\":{\"x\":1,\"x\":2}}"),"a"),"nested duplicate JSON");
        Refuses(()=>Contract.Json(Encoding.UTF8.GetBytes("{\"a\":1,\"b\":2}"),"a"),"unknown JSON");
        Refuses(()=>Contract.Json(Encoding.UTF8.GetBytes("{}"),"a"),"missing JSON");
        Equal("kernel32.dll",String.Join(",",PeImports.Read(Image())),"actual PE import");
        var malformed=Image();U32(malformed,524,0xffffffff);Refuses(()=>PeImports.Read(malformed),"unmapped import name");
        malformed=Image();U32(malformed,0x98+124,20);Refuses(()=>PeImports.Read(malformed),"missing import terminator");
        malformed=Image();U16(malformed,0x84,0x14c);Refuses(()=>PeImports.Read(malformed),"wrong architecture");
        malformed=Image();malformed[608]=(byte)'/';Refuses(()=>PeImports.Read(malformed),"path import");
        Contract.RequireBinding("same","same","copy hash");
        Refuses(()=>Contract.RequireBinding("reviewed","changed","copy hash"),"copy hash mismatch");
        foreach(string capability in new[]{"pre-existing launcher SeImpersonatePrivilege","Administrators default-owner eligibility","duplicate SeRestorePrivilege"}) {
            Contract.RequireCapability(true,capability);
            Refuses(()=>Contract.RequireCapability(false,capability),"unavailable "+capability);
        }
        var imports=new HashSet<string>{"kernel32.dll","api-ms-win-crt-runtime-l1-1-0.dll"};
        var modules=new HashSet<string>{"kernel32.dll","ucrtbase.dll"};
        var hosts=new Dictionary<string,string>{{"api-ms-win-crt-runtime-l1-1-0.dll","ucrtbase.dll"}};
        Contract.RequireImport("kernel32.dll",imports,modules,hosts);
        Contract.RequireImport("api-ms-win-crt-runtime-l1-1-0.dll",imports,modules,hosts);
        Refuses(()=>Contract.RequireImport("private.dll",imports,modules,hosts),"unreviewed import");
        Refuses(()=>Contract.RequireImport("kernel32.dll",imports,new HashSet<string>(),hosts),"missing fixed module");
        Refuses(()=>Contract.RequireImport("api-ms-win-crt-runtime-l1-1-0.dll",imports,modules,new Dictionary<string,string>()),"missing API contract");
        Refuses(()=>Contract.RequireImport("api-ms-win-crt-runtime-l1-1-0.dll",imports,modules,new Dictionary<string,string>{{"api-ms-win-crt-runtime-l1-1-0.dll","private.dll"}}),"redirected API contract");
        string identity="{\"volume\":\"1\",\"file_id\":\"00112233445566778899aabbccddeeff\"}";
        string cleanup="{\"cleanup\":{\"runtime_home_removed\":true,\"runtime_home_identity\":"+identity+"}}";
        using(var facts=Contract.Json(Encoding.UTF8.GetBytes(cleanup),"cleanup")) Contract.RequireCleanup(Contract.Filters[0],facts.RootElement);
        using(var facts=Contract.Json(Encoding.UTF8.GetBytes("{}"))) Refuses(()=>Contract.RequireCleanup(Contract.Filters[0],facts.RootElement),"missing cleanup proof");
        using(var facts=Contract.Json(Encoding.UTF8.GetBytes(cleanup.Replace(":true",":false")),"cleanup")) Refuses(()=>Contract.RequireCleanup(Contract.Filters[0],facts.RootElement),"false cleanup proof");
        using(var facts=Contract.Json(Encoding.UTF8.GetBytes(cleanup.Replace("\"1\"","\"01\"")),"cleanup")) Refuses(()=>Contract.RequireCleanup(Contract.Filters[0],facts.RootElement),"noncanonical cleanup identity");
        using(var facts=Contract.Json(Encoding.UTF8.GetBytes(cleanup),"cleanup")) Refuses(()=>Contract.RequireCleanup(Contract.Filters[2],facts.RootElement),"premature setup cleanup");
        var state=new SettlementState();Refuses(state.RequireClosable,"live child close");
        state.AcceptanceFailed();Equal(true,state.Failed,"deadline sticky failure");Equal(false,state.CleanupAllowed,"live child cleanup");
        state.SettlementFailed();Equal(true,state.Preserve,"failed settlement preserves");
        state.ObserveExit();state.RequireClosable();Equal(true,state.Failed,"late exit cannot green");Equal(false,state.CleanupAllowed,"late exit preserves diagnosis");
        // Every acquired resource is attempted despite failures at any earlier close.
        for(int fail=0;fail<8;fail++) {
            var attempted=new List<int>();var errors=new List<string>();var actions=new List<KeyValuePair<string,Action>>();
            for(int i=0;i<8;i++) { int slot=i;actions.Add(new KeyValuePair<string,Action>("resource-"+i,()=> { attempted.Add(slot);if(slot==fail) throw new IOException("injected close failure"); })); }
            CleanupBatch.Run(actions,(phase,error)=>errors.Add(phase));
            Equal("0,1,2,3,4,5,6,7",String.Join(",",attempted),"all independent cleanup attempted");Equal("resource-"+fail,String.Join(",",errors),"original cleanup phase retained");
        }
        Console.WriteLine("managed provisioner contract checks="+checks+" PASS; no native Windows claim");return 0;
    }
}
