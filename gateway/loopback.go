package main

import (
	"net"
	"net/url"
	"strings"
)

// loopbackAlias reports whether host names the same personal gateway as origin under another
// loopback spelling (localhost, 127.0.0.1, [::1]) on the same port. Such a visit is sent to
// the configured origin, so the session cookie and the host check both see one address.
func loopbackAlias(host string, origin *url.URL) bool {
	name, port, err := net.SplitHostPort(host)
	if err != nil || port != origin.Port() || strings.EqualFold(host, origin.Host) {
		return false
	}
	return loopbackName(name) && loopbackName(origin.Hostname())
}

func loopbackName(name string) bool {
	return strings.EqualFold(name, "localhost") || name == "127.0.0.1" || name == "::1"
}
