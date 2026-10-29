# Several boxes with the same project

This machine mirrors a box's lane: its proxy routes the lane's hostnames to the box, and it listens on the lane's ports. Boxes made from one image, such as forks of a golden snapshot, run the same lanes. So do boxes that run the same branch. Each of them runs `acme@0` with the same hostnames and the same port numbers. This machine stays connected to all of them, and one of them serves `acme@0` here at a time. You choose which, and switching takes a moment.

## One box per lane

Take two forks, `fork-a` and `fork-b`. Both run `acme@0`, with `web` at `https://web.acme.localhost` on port 20000 and Postgres on port 20012.

A hostname goes through the proxy here, which picks the target by its name. Both forks serve `web.acme.localhost`, so the name alone can't pick a fork. A port number on this machine leads to exactly one place. Anything that uses a port reaches whatever holds it, such as `psql -p 20012` or a page here that calls `http://127.0.0.1:20000`, the box's `services.web.url`. So only one of the forks can serve `acme@0` here.

Giving each box its own domain, such as `web.acme.fork-b.localhost`, wouldn't help. The hostnames would differ, but anything that uses a port would still reach whichever box holds it, without any warning. Renaming hostnames or remapping ports here would break the URLs the box already handed out.

## Switching instead of renaming

The box chose its hostnames and ports and gave them to its processes, which put them in URLs and cookies. A switch passes them through unchanged, so what the box generated keeps working here. Every reference to the lane, by hostname or by port, reaches the same box. Nothing changes on the boxes. The other boxes stay connected. The cost is that you see one box per lane at a time.

## Clashing lanes

Two lanes clash when their hostnames overlap: the same name, or a name under the other's wildcard. That is the case for a box lane and a lane on this machine that serve the same names, and for lanes of two projects whose names overlap. Only hostnames this machine would route count, so a name outside `localhost` and your `hostnames.tld` clashes with nothing.

The same lane on two boxes, such as `acme@0` on both forks, always clashes, even when their hostnames differ or it has none at all. Its ports would still lead to one box only.

Of the lanes that clash, this machine mirrors one, in this order:

1. A lane of this machine that runs.
2. The box lane you chose, while its box is connected.
3. A lane of this machine that is stopped.
4. The lane of the box that connected first.

The lane in use gets all its hostnames and ports. The others are skipped whole, including names and ports that don't clash.

## Choosing

In the TUI, a box lane joins the lanes row of the project with the same name: `@0  @0 fork-a  @0 fork-b  @1`. The lanes this machine does not mirror are grey. Select one and press `m` to make it the one in use. On this machine's own grey lane, `m` gives the hostnames back to it. When this machine's lane runs, `m` on a box lane asks to stop it, then switches. In the boxes view (`B`), `m` works on a box's skipped lane too.

From a shell:

```sh
devlanes remote use fork-b acme@0     # fork-b's acme@0 serves the hostnames
devlanes remote use --local acme@0    # this machine's acme@0 serves them again
devlanes remote ls                    # skipped lanes say which lane is in use
```

You can choose a box that is not connected right now. `remote use` then says `acme@0 will come from fork-b once it connects`, and the box takes the lane when it connects.

The latest choice wins. When the lane you choose clashes with a lane chosen before, such as another project's lane with overlapping names, that older choice is deleted, and `remote use` says so. `remote use --local` also deletes the choices that took the names of this machine's lane.

A choice stays across reconnects and daemon restarts. Another `remote use` replaces it. `devlanes remote disconnect <box>` deletes the box's choices, and so does the cleanup that forgets boxes not connected for `history.remote_hosts_retention`.

## During a switch

All of the lane's hostnames and ports move to the new box together. This machine keeps listening on the ports throughout, so no other program can take them meanwhile, and the routes switch in one step. A hostname is never routed to both boxes, or to neither. `remote use` returns once the routes and ports lead to the new box. If the new box's service is still starting, a request waits for it or gets the usual starting page.

Open connections to the old box close, database clients among them. That includes keep-alive connections and WebSockets. An answer already under way gets through first. Dev servers' HMR clients reconnect and reload the page, so an open tab moves to the new box by itself.

A chosen box that connects, or reconnects after a restart, takes the lane the same way, once this machine has its routes.

## Browser state

Every box serves the same origins, such as `https://web.acme.localhost`. The browser keeps one set of cookies, local storage, service workers and HTTP cache for that origin, and devlanes leaves them alone, since clearing them would sign you out. Forks of one image usually share secrets and seed users, so a session often keeps working on the new box. When a fork rejects what the other one set, sign in again or clear the site's data.

## Lanes running on this machine

A lane running here holds its ports, so it always keeps its hostnames. So does a lane whose own containers are up and publish a host port. Shared containers, and containers without a published port, hold nothing a box needs. A Compose service with `network_mode: host` publishes nothing, so it never counts. Choosing a box for such a lane, or for a box lane it clashes with, is refused with the commands to run: `devlanes stop --lane acme@0`, then `devlanes remote use fork-b acme@0`. `devlanes stop` leaves containers running, so the message also names the commands that stop exactly the lane's own containers, such as `docker compose -p acme-lane0 stop` for its Compose project and `docker stop devlanes-acme-lane0-cerbos` for a plain container. When only containers are up, it names just those.

Starting a lane here takes its hostnames and ports back as soon as the start passed its checks, whatever it runs. Once a unit, a task or one of its own containers runs, it deletes the choice of a box lane it clashes with, and any choice of its own lane, whether or not its box is connected. A `choice_cleared` notice says so, such as `acme@0 started on this machine, so fork-b's acme@0 is no longer mirrored or chosen.` Containers that came back by themselves, such as after a reboot, do the same, also when the daemon restarted, and the notice says the containers run. A start that fails before anything runs gives the box its lane back. Stopping the lane later doesn't bring the box back. Run `devlanes remote use` again.

## Overlapping port ranges

Forks of one image share their `ports.range`, so `remote connect` and `remote ls` warn on each of them that its range overlaps the other's. For the lanes both forks run, that is expected: one box serves each of them at a time. Different lanes on the two forks, such as lanes created after the fork, can get the same port numbers, and then only one of them is mirrored here. Give each box its own `ports.range` in its user config before such lanes get their ports.

## Two boxes at the same time

To work with two boxes side by side, use a different lane number or project on each box, so their hostnames differ. Their ports differ only when each box has its own `ports.range` before the lane gets its ports. A lane index per box, with ports that follow it, is planned in [issue #98](https://github.com/devlanes-run/devlanes/issues/98).

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
