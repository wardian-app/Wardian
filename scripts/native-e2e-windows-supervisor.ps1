param(
  [Parameter(Mandatory = $true)]
  [string] $Executable,
  [Parameter(Mandatory = $true)]
  [string] $ArgumentsJson,
  [string] $ReceiptPath,
  [string] $ReceiptRunId,
  [int] $ReceiptTargetIndex = -1,
  [string] $ReceiptTargetSha256
)

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Collections;
using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;

public static class WardianNativeE2eSupervisor
{
    private const uint CREATE_SUSPENDED = 0x00000004;
    private const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;
    private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
    private const uint INFINITE = 0xffffffff;
    private const int JobObjectExtendedLimitInformation = 9;
    private const int JobObjectBasicAccountingInformation = 1;

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

    [StructLayout(LayoutKind.Sequential)]
    private struct JOBOBJECT_BASIC_ACCOUNTING_INFORMATION
    {
        public long TotalUserTime;
        public long TotalKernelTime;
        public long ThisPeriodTotalUserTime;
        public long ThisPeriodTotalKernelTime;
        public uint TotalPageFaultCount;
        public uint TotalProcesses;
        public uint ActiveProcesses;
        public uint TotalTerminatedProcesses;
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
    private static extern bool GetProcessTimes(IntPtr hProcess, out long creation, out long exit, out long kernel, out long user);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool QueryInformationJobObject(IntPtr hJob, int infoClass,
        out JOBOBJECT_BASIC_ACCOUNTING_INFORMATION information, uint length, out uint returnedLength);

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

    public static int Run(string executable, string[] arguments, IDictionary receipt)
    {
        IntPtr job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateJobObject failed");

        PROCESS_INFORMATION process = new PROCESS_INFORMATION();
        bool joined = false;
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
            if (receipt != null)
            {
                receipt["root_pid"] = process.dwProcessId;
                long birth, exit, kernel, user;
                if (GetProcessTimes(process.hProcess, out birth, out exit, out kernel, out user))
                {
                    // FILETIME stays a decimal string: JSON numbers lose full birth precision.
                    receipt["root_birth_filetime"] = birth.ToString(CultureInfo.InvariantCulture);
                    receipt["root_birth_utc"] = DateTime.FromFileTimeUtc(birth).ToString("O", CultureInfo.InvariantCulture);
                }
                else receipt["birth_error"] = Marshal.GetLastWin32Error();
            }

            // Assignment happens while the root is suspended, before it can
            // fork. Every descendant is then covered by KILL_ON_JOB_CLOSE.
            if (!AssignProcessToJobObject(job, process.hProcess))
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "AssignProcessToJobObject failed");
            }
            if (receipt != null) receipt["job_assigned"] = true;
            if (ResumeThread(process.hThread) == 0xffffffff) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "ResumeThread failed");
            }
            if (receipt != null) receipt["root_resumed"] = true;
            uint waitResult = WaitForSingleObject(process.hProcess, INFINITE);
            int waitError = waitResult == 0xffffffff ? Marshal.GetLastWin32Error() : 0;
            if (receipt != null)
            {
                receipt["wait_result"] = waitResult;
                if (waitResult == 0xffffffff) receipt["wait_error"] = waitError;
            }
            if (waitResult == 0xffffffff) throw new Win32Exception(waitError, "WaitForSingleObject failed");
            if (waitResult != 0) throw new InvalidOperationException("Unexpected process wait result");
            joined = true;
            if (receipt != null) receipt["joined"] = true;
            uint exitCode;
            if (!GetExitCodeProcess(process.hProcess, out exitCode)) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "GetExitCodeProcess failed");
            }
            if (receipt != null) receipt["root_exit_code"] = exitCode;
            return unchecked((int)exitCode);
        }
        finally
        {
            try
            {
                if (process.hProcess != IntPtr.Zero)
                {
                    try
                    {
                        // Preserve the existing original-handle cleanup, even on receipt failure.
                        if (process.dwProcessId != 0) TerminateProcess(process.hProcess, 1);
                        if (receipt != null) receipt["root_terminate_before_join"] = !joined;
                    }
                    finally { CloseHandle(process.hProcess); }
                }
            }
            finally
            {
                if (process.hThread != IntPtr.Zero) CloseHandle(process.hThread);
                try
                {
                    if (receipt != null)
                    {
                        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION accounting;
                        uint returnedLength;
                        bool observed = QueryInformationJobObject(job, JobObjectBasicAccountingInformation,
                            out accounting, (uint)Marshal.SizeOf(typeof(JOBOBJECT_BASIC_ACCOUNTING_INFORMATION)), out returnedLength);
                        int queryError = observed ? 0 : Marshal.GetLastWin32Error();
                        if (observed)
                        {
                            receipt["active_processes_before_close"] = accounting.ActiveProcesses;
                            receipt["kill_on_close_residue_requested"] = accounting.ActiveProcesses > 0;
                        }
                        else receipt["job_query_error"] = queryError;
                    }
                }
                finally
                {
                    // This is the existing containment action. A positive pre-close
                    // count reports aggregate residue, never which process it killed.
                    bool closed = CloseHandle(job);
                    if (receipt != null)
                    {
                        receipt["job_close_called"] = true;
                        receipt["job_close_succeeded"] = closed;
                    }
                }
            }
        }
    }
}
"@

$receipt = $null
$receiptStream = $null
try {
    if ($ReceiptPath) {
        if (-not $ReceiptRunId -or $ReceiptRunId.Length -gt 256 -or $ReceiptRunId -match '[\x00-\x1f]' -or
            $ReceiptTargetIndex -lt 0 -or $ReceiptTargetSha256 -notmatch '^[a-f0-9]{64}$' -or
            $env:WARDIAN_E2E_RUN_ID -cne $ReceiptRunId -or -not $env:WARDIAN_E2E_NATIVE_HOME -or
            -not $env:WARDIAN_NATIVE_INNER_RECEIPT_DIRECTORY) {
            throw 'Incomplete inner-supervisor receipt scope'
        }
        $receiptHome = [IO.Path]::GetFullPath($env:WARDIAN_E2E_NATIVE_HOME).TrimEnd([IO.Path]::DirectorySeparatorChar)
        $receiptFile = [IO.Path]::GetFullPath($ReceiptPath)
        $receiptDirectory = [IO.Path]::GetFullPath($env:WARDIAN_NATIVE_INNER_RECEIPT_DIRECTORY).TrimEnd([IO.Path]::DirectorySeparatorChar)
        if (-not [IO.Path]::IsPathRooted($ReceiptPath) -or
            -not [IO.Path]::IsPathRooted($env:WARDIAN_NATIVE_INNER_RECEIPT_DIRECTORY) -or
            -not [IO.Path]::GetDirectoryName($receiptFile).Equals($receiptDirectory, [StringComparison]::OrdinalIgnoreCase) -or
            $receiptDirectory.Equals($receiptHome, [StringComparison]::OrdinalIgnoreCase) -or
            $receiptDirectory.StartsWith($receiptHome + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Inner-supervisor receipt must use the exact fresh output outside the resettable home'
        }
        $receiptOwner = Get-Content -Raw -LiteralPath (Join-Path $receiptHome '.native-e2e-lock/owner.json') | ConvertFrom-Json
        if ($receiptOwner.runId -cne $ReceiptRunId) { throw 'Inner-supervisor receipt home claim changed' }
        $receiptHomeItem = Get-Item -Force -LiteralPath $receiptHome
        if ($receiptHomeItem.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Inner-supervisor home traverses a link' }
        $receiptParent = $receiptDirectory
        while ($true) {
            $receiptItem = Get-Item -Force -LiteralPath $receiptParent
            if (-not $receiptItem.PSIsContainer -or ($receiptItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Inner-supervisor receipt directory traverses a link'
            }
            $nextReceiptParent = [IO.Path]::GetDirectoryName($receiptParent)
            if (-not $nextReceiptParent -or $nextReceiptParent -eq $receiptParent) { break }
            $receiptParent = $nextReceiptParent
        }
        $wireHasher = [Security.Cryptography.SHA256]::Create()
        try {
            $wireBytes = [Text.Encoding]::UTF8.GetBytes($Executable + [char]0 + $ArgumentsJson)
            $actualTargetSha256 = ([BitConverter]::ToString($wireHasher.ComputeHash($wireBytes))).Replace('-', '').ToLowerInvariant()
        } finally { $wireHasher.Dispose() }
        if ($actualTargetSha256 -cne $ReceiptTargetSha256) { throw 'Inner-supervisor target binding mismatch' }
        # Reserve before any child starts. Existing/stale files are never overwritten.
        $receiptStream = [IO.File]::Open($receiptFile, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $receipt = [ordered]@{
            schema = 1; kind = 'native-e2e-inner-supervisor'; run_id = $ReceiptRunId
            target_index = $ReceiptTargetIndex; target_sha256 = $actualTargetSha256
            root_pid = $null; root_birth_filetime = $null; root_birth_utc = $null; birth_error = $null
            job_assigned = $false; root_resumed = $false; wait_result = $null; wait_error = $null
            joined = $false; root_exit_code = $null; root_terminate_before_join = $null
            active_processes_before_close = $null; job_query_error = $null
            kill_on_close_residue_requested = $null; job_close_called = $false; job_close_succeeded = $null
            stdio_eof = 'not_observed_inherited'; run_returned = $false; observations_complete = $false
            writer_finished = $false; error_type = $null
        }
    }
    $parsedArguments = ConvertFrom-Json -InputObject $ArgumentsJson
    $arguments = @(foreach ($argument in $parsedArguments) { [string] $argument })
    [int] $exitCode = [WardianNativeE2eSupervisor]::Run($Executable, [string[]] $arguments, $receipt)
    if ($null -ne $receipt) { $receipt['run_returned'] = $true }
} catch {
    if ($null -ne $receipt) { $receipt['error_type'] = $_.Exception.GetType().Name }
    throw
} finally {
    if ($null -ne $receiptStream) {
        try {
            if ($null -ne $receipt) {
                $receipt['observations_complete'] = $receipt.run_returned -and $receipt.joined -and
                    $null -ne $receipt.root_birth_filetime -and $null -ne $receipt.root_exit_code -and
                    $null -ne $receipt.active_processes_before_close -and $receipt.job_close_succeeded
                $receipt['writer_finished'] = $true
                $receiptBytes = [Text.Encoding]::UTF8.GetBytes(($receipt | ConvertTo-Json -Depth 4 -Compress))
                $receiptStream.Write($receiptBytes, 0, $receiptBytes.Length)
                $receiptStream.Flush($true)
            }
        } finally { $receiptStream.Dispose() }
    }
}
exit $exitCode
