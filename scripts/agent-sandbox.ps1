<#
.DEPRECATED 2026-06 — SUPERSEDED BY THE THREE-AGENT MODEL.
  This script enforces a single-agent "branch-per-phase / never touch main" workflow and
  references rules (.cursor/rules/agent-sandbox.mdc, git-workflow.mdc) that no longer exist.
  RaeenOS now uses three peer agents committing to `main`, isolated by crate ownership and
  enforced by scripts/ownership-lock.sh + scripts/architecture-gate.sh (pre-commit hooks).
  See AGENTS.md "Three-agent parallel development", agents/OWNERSHIP.toml, and the per-agent
  files CLAUDE.md / .gemini/GEMINI.md / .cursor/rules/composer-slice.mdc.
  Retained for reference only. Do not use for new work.

.SYNOPSIS
  RaeenOS agent sandbox driver — wraps git-workflow.mdc as one-liners. (DEPRECATED)

.DESCRIPTION
  Every agent session should call into this script for branch and reset
  operations rather than running raw `git` commands. The script enforces
  the rules in .cursor/rules/agent-sandbox.mdc and git-workflow.mdc:

    - Refuses to operate on `main`.
    - Refuses to push without -AllowPush.
    - Stages explicit paths only.
    - Reports a session-end summary.

  Subagents get the same surface via the same script; they do not run raw
  `git reset --hard` themselves.

.PARAMETER Phase
  Phase number from MasterChecklist.md (e.g. 1, 2, 6).

.PARAMETER Slug
  Kebab-case phase slug (e.g. tier0-boot, tier1-useful, raegfx-vulkan).

.PARAMETER ItemId
  Optional. Checklist item id (e.g. 1.4-osipic). When set, work happens on
  phase/<n>-<slug>/item/<id>; when omitted, on phase/<n>-<slug>.

.PARAMETER Action
  bootstrap   - create the phase branch from origin/main (local-only)
  reset       - hard-reset the working branch to origin/main
  start-item  - create an item branch off the phase tip
  drop-item   - delete the item branch (after merging or giving up)
  status      - print the session-end audit summary
  prep-commit - stage the given -Path values and PRINT (do not run) the
                commit command; the user must explicitly say "commit".

.EXAMPLE
  pwsh scripts/agent-sandbox.ps1 -Phase 1 -Slug tier0-boot -Action bootstrap
  pwsh scripts/agent-sandbox.ps1 -Phase 1 -Slug tier0-boot -Action reset
  pwsh scripts/agent-sandbox.ps1 -Phase 1 -Slug tier0-boot -ItemId 1.4-osipic -Action start-item
  pwsh scripts/agent-sandbox.ps1 -Phase 1 -Slug tier0-boot -ItemId 1.4-osipic -Path kernel/src/acpi.rs,MasterChecklist.md -Action prep-commit
  pwsh scripts/agent-sandbox.ps1 -Phase 1 -Slug tier0-boot -Action status
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][int]$Phase,
    [Parameter(Mandatory=$true)][string]$Slug,
    [string]$ItemId,
    [ValidateSet('bootstrap','reset','start-item','drop-item','status','prep-commit')][string]$Action,
    [string[]]$Path,
    [string]$WorktreeRoot = "C:\Users\woisr\Worktrees",
    [switch]$AllowPush
)

$ErrorActionPreference = 'Stop'

function Branch-Name {
    param([int]$n, [string]$slug, [string]$item)
    $b = "phase/$n-$slug"
    if ($item) { "$b/item/$item" } else { $b }
}

function Assert-NotMain {
    $cur = git rev-parse --abbrev-ref HEAD
    if ($cur -eq 'main') {
        throw "Refusing to run '$Action' on branch 'main'. Switch to a phase branch first."
    }
    if ($cur -ne (Branch-Name $Phase $Slug $ItemId)) {
        Write-Warning "Current branch '$cur' does not match expected '$(Branch-Name $Phase $Slug $ItemId)'. Continuing anyway — caller is responsible."
    }
}

$PhaseBranch = Branch-Name $Phase $Slug
$ItemBranch  = if ($ItemId) { Branch-Name $Phase $Slug $ItemId } else { $null }
$WorkBranch  = if ($ItemBranch) { $ItemBranch } else { $PhaseBranch }
$Worktree    = Join-Path $WorktreeRoot "phase-$Phase"

switch ($Action) {
    'bootstrap' {
        git fetch origin | Out-Null
        git checkout main
        $dirty = git status --porcelain
        if ($dirty) { throw "Working tree dirty on main. Commit or stash first.`n$dirty" }
        git pull --ff-only origin main

        $exists = git branch --list $PhaseBranch
        if (-not $exists) {
            git checkout -b $PhaseBranch
            Write-Host "Created local branch $PhaseBranch from origin/main."
        } else {
            git checkout $PhaseBranch
            Write-Host "Checked out existing branch $PhaseBranch."
        }

        if ($AllowPush) {
            git push -u origin $PhaseBranch
        } else {
            Write-Host "Skipping push (no -AllowPush). Branch is local-only."
        }

        if (-not (Test-Path $Worktree)) {
            git worktree add $Worktree $PhaseBranch
            Write-Host "Created worktree at $Worktree."
        } else {
            Write-Host "Worktree already exists at $Worktree."
        }
    }

    'reset' {
        Assert-NotMain
        git fetch origin | Out-Null
        git restore .
        git clean -fdx
        git reset --hard origin/main
        $dirty = git status --porcelain
        $diff  = git diff origin/main
        if ($dirty -or $diff) {
            throw "Reset incomplete. Working tree still dirty or differs from origin/main."
        }
        Write-Host "Hard-reset $WorkBranch to origin/main. Tree clean."
    }

    'start-item' {
        if (-not $ItemId) { throw "-ItemId is required for start-item." }
        $exists = git branch --list $ItemBranch
        if ($exists) {
            git checkout $ItemBranch
            Write-Host "Checked out existing item branch $ItemBranch."
        } else {
            git checkout $PhaseBranch
            git pull --ff-only origin $PhaseBranch 2>$null   # ok if not on origin
            git checkout -b $ItemBranch
            Write-Host "Created item branch $ItemBranch off $PhaseBranch."
        }
    }

    'drop-item' {
        if (-not $ItemId) { throw "-ItemId is required for drop-item." }
        $cur = git rev-parse --abbrev-ref HEAD
        if ($cur -eq $ItemBranch) { git checkout $PhaseBranch }
        git branch -D $ItemBranch
        Write-Host "Deleted item branch $ItemBranch."
    }

    'status' {
        $cur      = git rev-parse --abbrev-ref HEAD
        $tip      = git rev-parse --short HEAD
        $ahead    = git rev-list --count origin/main..HEAD
        $aheadStr = "$ahead commits ahead of origin/main"
        $dirty    = (git status --porcelain) -join "`n"
        $wtList   = git worktree list --porcelain
        $treeStr  = if ($dirty) { 'dirty' } else { 'clean' }
        Write-Host ""
        Write-Host "Branch:        $cur"
        Write-Host "Tip:           $tip"
        Write-Host "Ahead:         $aheadStr"
        Write-Host "Working tree:  $treeStr"
        if ($dirty) { Write-Host $dirty }
        Write-Host "Worktrees:"
        Write-Host $wtList
        Write-Host ""
    }

    'prep-commit' {
        if (-not $Path -or $Path.Count -eq 0) {
            throw "-Path is required for prep-commit. Stage explicit paths only."
        }
        $cur = git rev-parse --abbrev-ref HEAD
        git add @Path
        Write-Host 'Staged:'
        git diff --cached --stat
        Write-Host ''
        Write-Host 'Next: draft the heredoc commit message in chat per master-checklist-workflow.mdc.'
        Write-Host 'DO NOT run git commit until the user types commit or ship it.'
        Write-Host 'DO NOT run git push until the user types push or ship it.'
        Write-Host ''
        $msg = 'Suggested push target: git push origin ' + $cur
        Write-Host $msg
    }
}
