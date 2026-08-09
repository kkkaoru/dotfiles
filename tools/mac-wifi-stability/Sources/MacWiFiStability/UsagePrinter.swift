internal enum UsagePrinter {
  internal static func printUsage() {
    print(
      """
      usage: mac-wifi-stability [--once|--status|--soft|--force|--force-rebind|--ohomemesh]
        --once          react to a launchd network configuration event
        --status        perform one explicit Wi-Fi/gateway diagnostic
        --soft          restart user-scope network agents; no logout/reboot
        --force         alias for --soft
        --force-rebind  compatibility alias for --soft
        --ohomemesh     connect once, then fallback to saved tethering if unhealthy
      """
    )
  }
}
