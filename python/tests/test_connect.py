from peer_lm.connect import AutoConnectTimers, ConnectMode, ResolvedConnect


def test_auto_forces_mdns_and_defers_relay_dial():
    r = ResolvedConnect.resolve(
        ConnectMode.AUTO, False, True, False, True, True, AutoConnectTimers()
    )
    assert r.enable_mdns
    assert r.enable_dcutr
    assert r.reserve_relay
    assert not r.dial_via_relay


def test_relay_mode_skips_mdns():
    r = ResolvedConnect.resolve(
        ConnectMode.RELAY, True, True, False, False, True, AutoConnectTimers()
    )
    assert not r.enable_mdns
    assert r.dial_via_relay
    assert r.reserve_relay
