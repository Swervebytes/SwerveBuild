param(
    [Parameter(Mandatory = $true)][ValidateSet("Home", "Projects", "Automations", "Memories", "Skills", "Settings")][string]$Page
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, "Swerve Build")
$win = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
if (-not $win) { throw "Swerve Build window not found" }

$nameCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, $Page)
$ctrlCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Hyperlink)
$and = New-Object System.Windows.Automation.AndCondition($nameCond, $ctrlCond)
$link = $null
for ($i = 0; $i -lt 20 -and -not $link; $i++) {
    $link = $win.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $and)
    if (-not $link) { Start-Sleep -Milliseconds 500 }
}
if (-not $link) { throw "Link '$Page' not found" }

$pattern = $link.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
$pattern.Invoke()