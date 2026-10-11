# Runs inside the guest's logged-on desktop (scheduled task /IT).
# Reads C:\ProgramData\SwerveBuild\request.json
# Writes result.json and, on success, screen.png
$ErrorActionPreference = 'Stop'
$root = 'C:\ProgramData\SwerveBuild'
$req = Get-Content (Join-Path $root 'request.json') -Raw | ConvertFrom-Json

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class SwerveMouse {
  [DllImport("user32.dll")]
  public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
}
'@

function Save-Shot {
  $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
  if ($bounds.Width -lt 1 -or $bounds.Height -lt 1) {
    throw 'desktop has no pixels (this task is not on the logged-on screen)'
  }
  $bmp = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
  $bmp.Save((Join-Path $root 'screen.png'), [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose()
  $bmp.Dispose()
}

function Find-Control([string]$name) {
  $desktop = [System.Windows.Automation.AutomationElement]::RootElement
  $cond = New-Object System.Windows.Automation.PropertyCondition (
    [System.Windows.Automation.AutomationElement]::NameProperty, $name)
  $el = $desktop.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
  if ($null -eq $el) { throw "no control named '$name'" }
  return $el
}

function Click-Element($el) {
  try { $el.SetFocus() | Out-Null } catch { }
  try {
    $pattern = $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $pattern.Invoke()
    return
  } catch { }
  $rect = $el.Current.BoundingRectangle
  if ($rect.Width -lt 1 -or $rect.Height -lt 1) {
    throw 'control has no Invoke pattern and no clickable bounds'
  }
  $x = [int]($rect.X + ($rect.Width / 2))
  $y = [int]($rect.Y + ($rect.Height / 2))
  [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point $x, $y
  [SwerveMouse]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
  [SwerveMouse]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
}

function Escape-SendKeys([string]$text) {
  $out = ''
  foreach ($ch in $text.ToCharArray()) {
    if ('+^%~(){}[]'.Contains([string]$ch)) { $out += '{' + $ch + '}' }
    else { $out += $ch }
  }
  return $out
}

try {
  switch ([string]$req.action) {
    'screenshot' { Save-Shot }
    'invoke' {
      Click-Element (Find-Control ([string]$req.controlName))
      Start-Sleep -Milliseconds 400
      Save-Shot
    }
    'type' {
      $el = Find-Control ([string]$req.controlName)
      try { $el.SetFocus() | Out-Null } catch { }
      $written = $false
      try {
        $value = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        $value.SetValue([string]$req.text)
        $written = $true
      } catch { $written = $false }
      if (-not $written) {
        [System.Windows.Forms.SendKeys]::SendWait((Escape-SendKeys ([string]$req.text)))
      }
      Start-Sleep -Milliseconds 200
      Save-Shot
    }
    'press' {
      if ([string]$req.controlName) {
        try { (Find-Control ([string]$req.controlName)).SetFocus() | Out-Null } catch { }
      }
      $send = switch ([string]$req.key) {
        'Enter' { '{ENTER}' }
        'Tab' { '{TAB}' }
        'Escape' { '{ESC}' }
        'Backspace' { '{BACKSPACE}' }
        'Space' { ' ' }
        'Up' { '{UP}' }
        'Down' { '{DOWN}' }
        'Left' { '{LEFT}' }
        'Right' { '{RIGHT}' }
        default { throw "key not allowed: $($req.key)" }
      }
      [System.Windows.Forms.SendKeys]::SendWait($send)
      Start-Sleep -Milliseconds 200
      Save-Shot
    }
    default { throw "unknown guest action $($req.action)" }
  }
  @{ ok = $true; action = [string]$req.action } | ConvertTo-Json -Compress |
    Set-Content (Join-Path $root 'result.json') -Encoding utf8
} catch {
  @{ ok = $false; error = $_.Exception.Message } | ConvertTo-Json -Compress |
    Set-Content (Join-Path $root 'result.json') -Encoding utf8
  exit 1
}
