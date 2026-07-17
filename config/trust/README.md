# Browser trust anchors

`mozilla-addons-public.pem` is copied verbatim from Firefox commit
`79223b6429f6d8844435cfdbf68ad699367bfec5`, path
`security/manager/ssl/addons-public.pem`. Update only with Firefox pin review;
never substitute operating-system Web PKI roots for extension signing.

`mozilla-addons-public-intermediates.pem` combines Firefox's pinned
`addons-public-intermediate.pem` and `addons-public-2018-intermediate.pem`
from that same commit. Firefox preloads both for stable AMO path building.
