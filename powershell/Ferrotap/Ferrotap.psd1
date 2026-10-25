@{
    RootModule        = 'Ferrotap.psm1'
    ModuleVersion     = '0.1.0'
    GUID              = '6b1f6a52-3c1e-4c58-9a0e-5d0f3f1a7c01'
    Author            = 'Ferrotap'
    Description       = 'PowerShell cmdlets to create, test and dry-run Ferrotap Auto Responder rules and breakpoint filters.'
    PowerShellVersion = '5.1'
    FunctionsToExport = @(
        'New-FerrotapRule', 'Get-FerrotapRule', 'Save-FerrotapRule',
        'Test-FerrotapRule', 'Test-FerrotapFilter', 'ConvertFrom-FerrotapFilter',
        'Get-FerrotapRuleResponse', 'Invoke-FerrotapTap'
    )
    CmdletsToExport   = @()
    VariablesToExport = @()
    AliasesToExport   = @()
}
