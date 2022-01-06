console.log("Oh my!");

let button = document.querySelector("button");


const triggerAuthFlow = () => {

    let keypairP = crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [ "sign" ]);
    keypairP.then((keypair) => {

        console.log(`Generated keypair`);

        window.addEventListener('message', onFirstMessage);

        // TODO: add timeout
        let identityWindow = window.open("https://identity.ic0.app/#authorize");

        console.log(`Opened window! ${identityWindow}`);

    });
}

const onFirstMessage = (message) => {
            console.log(`Got message! ${JSON.stringify(message.data)}`);

            // TODO: also check everything is well formatted (fields exist)
            if(message.data.kind !== "authorize-ready") {
                console.log("Something bad happened");
                return;
            }
            onAuthorizeReady();

}

const onAuthorizeReady = () => {
            let sessionPublicKeyP = crypto.subtle.exportKey("raw", keypair.publicKey);

            sessionPublicKeyP.then((sessionPublicKey) => {
                let msg = {
                    kind: "authorize-client",
                    sessionPublicKey: sessionPublicKey,
                };

                identityWindow.postMessage(msg, "https://identity.ic0.app");
            });
}

button.onclick = triggerAuthFlow;

