# sv-terms-03 · Kubernetes och Helm

Kategori: sv-terms · Språk: sv · Mål: ca 105 s
Termer: deploy, container, Helm, pipeline, staging, Kubernetes, rollback (föreslagen ny term).

---
I: Hur ser en vanlig deploy ut hos er?
G: Varje tjänst byggs som en container och paketeras med Helm. När en utvecklare mergar till huvudgrenen bygger vår pipeline en ny image, taggar den med versionsnumret och uppdaterar chartet i Helm.
I: Och sedan går den direkt ut?
G: Nej, först går den till staging. Där kör vi ett antal automatiska tester mot klustret i staging. Om testerna går igenom kan man flytta versionen till produktion med en knapptryckning.
I: Vad händer om något går fel i produktion?
G: Då gör vi en rollback med Helm. Det tar ungefär en minut, och Kubernetes ser till att de nya containrarna inte tar emot trafik förrän de gamla har ersatts, så användarna märker oftast ingenting.
I: Vad har varit svårast med Kubernetes?
G: Att få utvecklarna att förstå varför deras container startar om hela tiden. Oftast handlar det om att man har satt för lite minne. Vi har skrivit en kort guide om det, och sedan dess har frågorna i vår kanal blivit betydligt färre.
I: Skulle du välja Kubernetes igen?
G: För oss, ja. Men för ett litet team med två tjänster hade jag nog valt något enklare. Det är lätt att underskatta hur mycket jobb det är att drifta ett kluster.
