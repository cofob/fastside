# fastside

A smart redirecting gateway for various frontend services. Faster and compatible
alternative to farside.

Contents

- [fastside](#fastside)
  - [About](#about)
  - [Demo](#demo)
  - [Mirrors](#mirrors)
  - [How It Works](#how-it-works)
  - [Why does this fork exist?](#why-does-this-fork-exist)

## About

A redirecting service for FOSS alternative frontends.

[Fastside](https://fastside.link) provides links that automatically redirect to
working instances of privacy-oriented alternative frontends, such as Nitter,
Libreddit, etc. This allows for users to have more reliable access to the
available public instances for a particular service, while also helping to
distribute traffic more evenly across all instances and avoid performance
bottlenecks and rate-limiting.

## Demo

Fastside's links work with the following structure: `fastside.link/<service>/<path>`

For example:

<table>
    <tr>
        <td>Service</td>
        <td>Page</td>
        <td>Fastside Link</td>
    </tr>
    <tr>
        <td><a href="https://github.com/spikecodes/libreddit">Libreddit</a></td>
        <td>/r/popular</td>
        <td><a href="https://fastside.link/libreddit/r/popular">https://fastside.link/libreddit/r/popular</a></td>
    </tr>
    <tr>
        <td><a href="https://codeberg.org/teddit/teddit">Teddit</a></td>
        <td>/r/popular</td>
        <td><a href="https://fastside.link/teddit/r/popular">https://fastside.link/teddit/r/popular</a></td>
    </tr>
    <tr>
        <td><a href="https://github.com/iv-org/invidious">Invidious</a></td>
        <td>/watch?v=zLGDE2j_n5c</td>
        <td><a href="https://fastside.link/_/invidious/watch?v=zLGDE2j_n5c">https://fastside.link/_/invidious/watch?v=zLGDE2j_n5c</a></td>
    </tr>
    <tr>
        <td><a href="https://github.com/iv-org/invidious">Invidious</a></td>
        <td>https://www.youtube.com/watch?v=zLGDE2j_n5c</td>
        <td><a href="https://fastside.link/https://www.youtube.com/watch?v=zLGDE2j_n5c">https://fastside.link/https://www.youtube.com/watch?v=zLGDE2j_n5c</a></td>
    </tr>
    <tr>
        <td><a href="https://github.com/httpjamesm/AnonymousOverflow">AnonymousOverflow</a></td>
        <td>/questions/1732348/regex-match-open-tags-except-xhtml-self-contained-tags</td>
        <td><a href="https://fastside.link/@cached/anonymousoverflow/#questions/1732348/regex-match-open-tags-except-xhtml-self-contained-tags">https://fastside.link/@cached/anonymousoverflow/#questions/1732348/regex-match-open-tags-except-xhtml-self-contained-tags</a></td>
    </tr>
    <!-- more rows can be added as needed -->
</table>

<sup>Note: This table doesn't include all available services. For a complete list of supported frontends, see: https://fastside.link/</sup>

## Mirrors

Fastside can be opened in [clearnet](https://fastside.link/), [tor]() or [i2p]().

## How It Works

The app runs with an internally scheduled cron task that queries all instances
for services defined in [services.json](./services.json) every 5 minutes. For
each instance, as long as the instance takes <2 seconds to respond and returns
a successful response code, the instance is added to a list of available
instances for that particular service. If not, it is discarded until the next
update period.

Fastside's routing is minimal, similar to [Farside](https://github.com/benbusby/farside), but includes
an additional `/@cached/<service>#<path>` endpoint, which utilizes browser caching to achieve instant
redirects without waiting for server responses.

## Why does this fork exist?

[Farside](https://github.com/benbusby/farside) operates very slowly for some reason. The ping from my machine to
their server in the USA is 300 ms, and a redirect request takes about 1 second to process (!). This means that
processing a redirect takes 700 ms, which is incredibly long for such a simple task. On the other hand, Fastside
processes requests in 200-300 ms (taking my internet into account). Additionally, the web server at fastside.link
supports http3, which saves us an additional 100-150 ms.
