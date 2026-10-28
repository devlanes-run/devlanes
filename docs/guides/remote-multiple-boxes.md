# Several boxes with the same project

Boxes made from one image, such as forks of a golden snapshot, run the same lanes. So do boxes that run the same branch. Each of them runs `acme@0` with the same hostnames and the same port numbers. This machine stays connected to all of them, and one of them serves `acme@0` here at a time. You choose which, and switching takes a moment.

## One box per lane

Take two forks, `fork-a` and `fork-b`. Both run `acme@0`, with `web` at `https://web.acme.localhost` on port 20000 and Postgres on port 20012.

A hostname goes through the proxy here, which picks the target by its name. A port number on this machine leads to exactly one place. Everything that uses a port reaches whatever holds it: a page that calls `http://127.0.0.1:20000`, the `services.web.url` another service got, `psql -p 20012`, a debugger. So only one of the forks can serve `acme@0` here.

Giving each box its own domain, such as `web.acme.fork-b.localhost`, wouldn't help. The hostnames would differ, but anything that uses a port would still reach whichever box holds it, without any warning. Remapping ports or addresses here would break the URLs the box already gave its processes, its CORS origins and its cookies.

## Why switching

The box chose its hostnames and ports and gave them to its processes, its URLs, its CORS origins and its cookies. A switch passes all of it through unchanged, so everything the box generated keeps working here.

Every reference to a lane reaches the same box, by hostname or by port. A lane is never half on one box and half on another.

The boxes don't change. Nothing restarts on them, and their lanes keep their numbers, ports and data. A box never takes orders from this machine.

The other boxes stay connected and visible. A switch takes a moment and reconnects nothing.

The cost is that you see one box per lane at a time.

## What clashes

Two lanes clash when their hostnames overlap: the same name, or a name under the other's wildcard. That is the case for a lane on two forks, and for a box lane and a lane on this machine that serve the same names. Only hostnames this machine would route count, so a name outside `localhost` and your `hostnames.tld` clashes with nothing.

Of the lanes that clash, this machine mirrors one, in this order:

1. A lane of this machine that runs.
2. The box lane you chose, while its box is connected.
3. A lane of this machine that is stopped.
4. The lane of the box that connected first.

The lane in use gets all its hostnames and ports. The others are skipped whole. Without a choice, a stopped lane here keeps its hostnames, and the box connected first keeps the rest.

## Choosing

In the TUI, a box lane joins the lanes row of the project with the same name: `@0  @0 fork-a  @0 fork-b  @1`. The lanes this machine does not mirror are grey. Select one and press `m` to make it the one in use. On this machine's own grey lane, `m` gives the hostnames back to it.

From a shell:

```sh
devlanes remote use fork-b acme@0     # fork-b's acme@0 serves the hostnames
devlanes remote use --local acme@0    # this machine's acme@0 serves them again
devlanes remote ls                    # skipped lanes say which lane is in use
```

You can choose a box that is not connected right now. `remote use` then says `acme@0 will come from fork-b once it connects`, and the box takes the lane when it connects.

A choice stays across reconnects and daemon restarts. Another `remote use` replaces it. `devlanes remote disconnect <box>` deletes the box's choices, and so does the cleanup that forgets boxes not connected for `history.remote_hosts_retention`.

## What a switch moves

All of the lane's hostnames and ports move to the new box together. This machine keeps listening on the ports throughout, so no other program can take them meanwhile, and the routes switch in one step. A request is never sent to both boxes, or to neither. `remote use` returns once the routes and ports lead to the new box. If the new box's service is still starting, a request waits for it or gets the usual starting page.

Open connections to the old box close. That covers database clients, keep-alive connections and WebSockets. An answer already under way gets through first. Dev servers' HMR clients reconnect and reload the page, so an open tab moves to the new box by itself.

## What stays in the browser

Every box serves the same origins, such as `https://web.acme.localhost`. The browser keeps one set of cookies, local storage, service workers and HTTP cache for that origin, and devlanes can't clear them without signing you out. Forks of one image usually share secrets and seed users, so a session often keeps working on the new box. When a fork rejects what the other one set, sign in again or clear the site's data.

## Lanes running on this machine

A lane running here holds its ports, so it always keeps its hostnames. Choosing a box for it is refused with the commands to run: `devlanes stop --lane acme@0`, then `devlanes remote use fork-b acme@0`. Starting a lane here takes its hostnames back at once, whatever the start runs, and deletes a choice it clashes with, with a `choice_cleared` notice.

## Two boxes at the same time

To work with two boxes side by side, give them different lanes or projects on the boxes themselves, so their hostnames and ports differ. A lane index per box, with ports that follow it, is planned in [issue #98](https://github.com/devlanes-run/devlanes/issues/98).

## Throwaway boxes

Forks that come and go are easier with two settings.

When SSH already decides who may reach a fork, set `remote.require_pairing: false` in the image's user config, so a new fork connects without a pairing code:

```yaml
# ~/.devlanes/config.yaml on the image
remote:
  require_pairing: false
```

When a fork is deleted, run `devlanes remote disconnect <fork>` here. Otherwise this machine keeps trying to reconnect it after every restart, and keeps its choices.

See [Remote boxes](../remote.md) for how mirroring works, [Remote setup](remote-setup.md) for connecting a box and [Hostnames and HTTPS](https.md#open-services-by-hostname) for why services open by hostname.
