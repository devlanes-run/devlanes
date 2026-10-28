# Several boxes with the same project

Boxes made from one image, such as forks of a golden snapshot, all run the same lanes. Each fork runs `acme@0` with the same hostnames and the same port block. This machine can stay connected to all of them, but it mirrors one fork's lane at a time, and you choose which.

## What clashes

Two lanes clash when their hostnames overlap: the same name, or a name under the other's wildcard, as the proxy matches them. This is the case for a lane on two forks, and for a box lane and a lane on this machine that serve the same names. Only hostnames this machine would route count, so a name outside `localhost` and your `hostnames.tld` clashes with nothing.

Of the lanes that clash, this machine mirrors one, in this order:

1. A lane of this machine that runs.
2. The box lane you chose, while its box is connected.
3. A lane of this machine that is stopped.
4. The lane of the box that connected first.

The lane in use gets the hostnames and the ports. The others are skipped whole, ports included, so a fork's lane never gets half of its names. Without a choice nothing changes: a stopped lane here keeps its hostnames, and the box connected first keeps the rest.

A box counts as connected while its link reconnects or waits for a sign-in, since its listeners and routes stay meanwhile. After a daemon restart, a chosen box takes its lane over again once the restore has connected it.

## Choosing

In the TUI, a box lane joins the lanes row of the project with the same name: `@0  @0 fork-a  @0 fork-b  @1`. The lanes this machine does not mirror are grey. Select one and press `m` to make it the one in use. On this machine's own grey lane, `m` gives the hostnames back to it.

From a shell:

```sh
devlanes remote use fork-b acme@0     # fork-b's acme@0 serves the hostnames
devlanes remote use --local acme@0    # this machine's acme@0 serves them again
devlanes remote ls                    # skipped lanes say which lane is in use
```

`remote use` answers once the switch is done: the old lane's ports are closed, with the connections open through them, and the new lane's ports and hostnames are in place. A hostname never points at two boxes during the switch.

A lane that runs on this machine always keeps its hostnames, so choosing a box for it is refused until you stop it. Starting a lane here takes its hostnames back at once and deletes a choice it clashes with, with a `choice_cleared` notice.

A choice stays across reconnects and daemon restarts. Another `remote use` replaces it. `devlanes remote disconnect <box>` deletes the box's choices, and so does the cleanup that forgets boxes not connected for `history.remote_hosts_retention`. While the chosen box is not connected, the order above applies without it.

## In the browser

Every fork serves the same origins, such as `https://web.acme.localhost`. The browser keeps one set of cookies, local storage and service workers for that origin, so they carry over a switch. A session cookie from fork-a reaches fork-b, which may not know it. Sign in again, or clear the site's data, when a fork rejects what the other one set.

Only one fork is open at a time. Two forks side by side would need different hostnames.

## Throwaway boxes

Forks that come and go are easier with two settings.

When SSH already decides who may reach a fork, set `remote.require_pairing: false` in the image's user config, so a new fork connects without a pairing code:

```yaml
# ~/.devlanes/config.yaml on the image
remote:
  require_pairing: false
```

When a fork is deleted, run `devlanes remote disconnect <fork>` here. Otherwise this machine keeps trying to reconnect it after every restart, and keeps its choices.

See [Remote boxes](../remote.md) for how mirroring works and [Remote setup](remote-setup.md) for connecting a box.
