# Looks at Windows' own passkey sheet ("Credential Dialog Xaml Host") with UI Automation. Used by
# extension/e2e/windows-provider.ts to close a sheet for "localhost" that a killed test browser left
# open. Needs no administrator rights and never touches the registry, a process or any other dialog.
#
#   -Mode dump      print every Windows Security dialog that is open (names, button ids only)
#   -Mode cleanup   press Cancel on passkey sheets whose text is "Passkey for localhost" (and only those)
#
# Windows shows the sheet as a "Credential Dialog Xaml Host" window, or as a child of the full-screen
# "Shell_SystemDim" window, which is why both places are searched. Prints one JSON line.
param(
    [ValidateSet('dump', 'cleanup')][string]$Mode = 'dump'
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text;
public class LpFg { [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow(); [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n); }
"@
$UIA = [System.Windows.Automation.AutomationElement]
$Scope = [System.Windows.Automation.TreeScope]
$Class = 'Credential Dialog Xaml Host'

function Get-Dialogs {
    $cond = New-Object System.Windows.Automation.PropertyCondition($UIA::ClassNameProperty, $Class)
    $found = New-Object System.Collections.ArrayList
    foreach ($d in $UIA::RootElement.FindAll($Scope::Children, $cond)) { [void]$found.Add($d) }
    $dim = New-Object System.Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'Shell_SystemDim')
    foreach ($w in $UIA::RootElement.FindAll($Scope::Children, $dim)) {
        foreach ($d in $w.FindAll($Scope::Descendants, $cond)) { [void]$found.Add($d) }
    }
    if ($found.Count -eq 0) {
        # Last resort: the dialog in front.
        $h = [LpFg]::GetForegroundWindow(); $sb = New-Object Text.StringBuilder 100; [void][LpFg]::GetClassName($h, $sb, 100)
        if ($sb.ToString() -eq $Class) { [void]$found.Add($UIA::FromHandle($h)) }
    }
    $found.ToArray()
}

function Get-Items($dialog) {
    foreach ($e in $dialog.FindAll($Scope::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
        $c = $e.Current
        [pscustomobject]@{ Element = $e; Type = ($c.ControlType.ProgrammaticName -replace '^ControlType\.', ''); Name = $c.Name; Id = $c.AutomationId }
    }
}

# A passkey sheet names the passkey ("Passkey for <site>"); the Windows Hello check does not.
function Describe($d) {
    $items = @(Get-Items $d)
    $texts = @($items | Where-Object { $_.Name -and $_.Type -in 'Text', 'Hyperlink' } | ForEach-Object { $_.Name })
    [pscustomobject]@{ Items = $items; Texts = $texts; IsSheet = [bool]($texts | Where-Object { $_ -like 'Passkey for *' }) }
}

function Out-Json($o) { $o | ConvertTo-Json -Compress -Depth 5 }

if ($Mode -eq 'dump') {
    $all = @(Get-Dialogs | ForEach-Object { Describe $_ })
    Out-Json @{ mode = 'dump'; dialogs = $all.Count; list = @($all | ForEach-Object { @{ sheet = $_.IsSheet; texts = $_.Texts; buttons = @($_.Items | Where-Object { $_.Type -eq 'Button' } | ForEach-Object { "$($_.Name)#$($_.Id)" }) } }) }
}
else {
    $n = 0
    for ($i = 0; $i -lt 30; $i++) {
        $mine = @(Get-Dialogs | ForEach-Object { Describe $_ } | Where-Object { $_.IsSheet -and ($_.Texts -contains 'Passkey for localhost') })
        if ($mine.Count -eq 0) { break }
        $cancel = $mine[0].Items | Where-Object { $_.Id -eq 'CancelButton' } | Select-Object -First 1
        if (-not $cancel) { break }
        try { $cancel.Element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() } catch { break }
        $n++
        Start-Sleep -Milliseconds 700
    }
    Out-Json @{ mode = 'cleanup'; cancelled = $n }
}
