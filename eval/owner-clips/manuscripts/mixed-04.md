# mixed-04 · Incident review

Kategori: mixed · Dominant language: sv · Mål: ca 100 s
Prövar: längre repliker med språkbyte vid varje replik. I talar svenska, G engelska.

---
I [sv]: Vi tar en snabb genomgång av incidenten i tisdags. Kan du börja med att berätta vad som hände?
G [en]: Sure. At around ten past two, customers started reporting that they couldn't log in. Our monitoring picked it up a few minutes later. It turned out that a certificate on the login service had expired during the night.
I [sv]: Hur kunde det hända? Vi har ju larm för certifikat som håller på att gå ut.
G [en]: We do, but this one was created manually a year ago, outside our normal process. So it was never added to the monitoring. Nobody knew it existed.
I [sv]: Hur lång tid tog det att lösa? Och hur många kunder märkte av det, tror du?
G [en]: About forty minutes from the first report. Most of that time went to finding the problem. Once we knew it was the certificate, replacing it took less than five minutes.
I [sv]: Vad gör vi för att det inte ska hända igen? Jag vill inte behöva förklara samma sak för ledningen en gång till.
G [en]: Two things. First, we're going through every service to find certificates that aren't in the monitoring. Second, we're moving all certificates to automatic renewal, so nobody has to remember them at all.
I [sv]: Bra. Kan du skriva ihop det och skicka till ledningsgruppen innan fredag? Ta gärna med tidslinjen också, så att de ser hur snabbt vi faktiskt agerade.
G [en]: Yes, I'll have it done by Thursday.
