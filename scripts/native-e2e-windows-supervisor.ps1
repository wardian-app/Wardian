param(
  [Parameter(Mandatory = $true)]
  [string] $Executable,
  [Parameter(Mandatory = $true)]
  [string] $ArgumentsJson
)

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

public static class WardianNativeE2eSupervisor
{
    private const uint CREATE_SUSPENDED = 0x00000004;
    private const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;
    private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
    private const uint INFINITE = 0xffffffff;
    private const int JobObjectExtendedLimitInformation = 9;

    [StructLayout(LayoutKind.Sequential)]
    private struct STARTUPINFO
    {
        public int cb;
        public IntPtr lpReserved;
        public IntPtr lpDesktop;
        public IntPtr lpTitle;
        public int dwX;
        public int dwY;
        public int dwXSize;
        public int dwYSize;
        public int dwXCountChars;
        public int dwYCountChars;
        public int dwFillAttribute;
        public int dwFlags;
        public short wShowWindow;
        public short cbReserved2;
        public IntPtr lpReserved2;
        public IntPtr hStdInput;
        public IntPtr hStdOutput;
        public IntPtr hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct PROCESS_INFORMATION
    {
        public IntPtr hProcess;
        public IntPtr hThread;
        public int dwProcessId;
        public int dwThreadId;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct JOBOBJECT_BASIC_LIMIT_INFORMATION
    {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IO_COUNTERS
    {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong OtherOperationCount;
        public ulong ReadTransferCount;
        public ulong WriteTransferCount;
        public ulong OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcess(
        string lpApplicationName,
        StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes,
        IntPtr lpThreadAttributes,
        bool bInheritHandles,
        uint dwCreationFlags,
        IntPtr lpEnvironment,
        string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo,
        out PROCESS_INFORMATION lpProcessInformation);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr lpJobAttributes, string lpName);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(
        IntPtr hJob,
        int JobObjectInfoClass,
        ref JOBOBJECT_EXTENDED_LIMIT_INFORMATION lpJobObjectInfo,
        uint cbJobObjectInfoLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint ResumeThread(IntPtr hThread);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr hObject);

    private static string Quote(string value)
    {
        if (value.Length == 0) return "\"\"";
        bool needsQuotes = false;
        foreach (char c in value)
        {
            if (char.IsWhiteSpace(c) || c == '\"') { needsQuotes = true; break; }
        }
        if (!needsQuotes) return value;

        var result = new StringBuilder("\"");
        int slashes = 0;
        foreach (char c in value)
        {
            if (c == '\\') { slashes++; continue; }
            if (c == '\"')
            {
                result.Append('\\', slashes * 2 + 1);
                result.Append(c);
                slashes = 0;
                continue;
            }
            result.Append('\\', slashes);
            result.Append(c);
            slashes = 0;
        }
        result.Append('\\', slashes * 2);
        result.Append('\"');
        return result.ToString();
    }

    public static int Run(string executable, string[] arguments)
    {
        IntPtr job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateJobObject failed");

        PROCESS_INFORMATION process = new PROCESS_INFORMATION();
        try
        {
            var limits = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, ref limits,
                (uint)Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION))))
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "SetInformationJobObject failed");
            }

            var command = new StringBuilder(Quote(executable));
            foreach (string argument in arguments) command.Append(' ').Append(Quote(argument));
            var startup = new STARTUPINFO();
            startup.cb = Marshal.SizeOf(typeof(STARTUPINFO));
            if (!CreateProcess(null, command, IntPtr.Zero, IntPtr.Zero, false,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT, IntPtr.Zero, null,
                ref startup, out process))
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateProcess failed");
            }

            // Assignment happens while the root is suspended, before it can
            // fork. Every descendant is then covered by KILL_ON_JOB_CLOSE.
            if (!AssignProcessToJobObject(job, process.hProcess))
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "AssignProcessToJobObject failed");
            }
            if (ResumeThread(process.hThread) == 0xffffffff) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "ResumeThread failed");
            }
            WaitForSingleObject(process.hProcess, INFINITE);
            uint exitCode;
            if (!GetExitCodeProcess(process.hProcess, out exitCode)) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "GetExitCodeProcess failed");
            }
            return unchecked((int)exitCode);
        }
        finally
        {
            if (process.hProcess != IntPtr.Zero)
            {
                if (process.dwProcessId != 0) TerminateProcess(process.hProcess, 1);
                CloseHandle(process.hProcess);
            }
            if (process.hThread != IntPtr.Zero) CloseHandle(process.hThread);
            // Closing the job is the authoritative cleanup. It terminates any
            // surviving descendant even after the root process has exited.
            CloseHandle(job);
        }
    }
}
"@

$parsedArguments = ConvertFrom-Json -InputObject $ArgumentsJson
$arguments = foreach ($argument in $parsedArguments) { [string] $argument }
[int] $exitCode = [WardianNativeE2eSupervisor]::Run($Executable, [string[]] $arguments)
exit $exitCode
