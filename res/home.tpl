<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="X-UA-Compatible" content="ie=edge">
    <title>BottledDiscord</title>
    
    <link rel="stylesheet" href="style/main.css">

    <style>
        .main {
            display: flex;
            flex-direction: column;
            height: 100%;
        }

        .navbar, .footer {
            flex-basis: 0.2;
        }

        .content {
            margin: 10%;
            display: flex;
            flex-direction: row;
            justify-content: space-between;
        }

        .desc {
            display: flex;
            flex-direction: column;
        }
    </style>
</head>
<body>
    <div class="main" >
        <div class="navbar" > navVV </div>
        
        <div class="content" >
            <div class="desc">
                <h1>Big thing</h1>
                <p>...does big stuff</p>    
                
                <div class="stats" >{{ bottlecount }} bottles, {{ usercount }} active users, and {{ guildcount }} servers.</div>
            </div>
            <div class="bottle" ><img id="bottle" src="img/bottle.png" ></div>
        </div>

        <div class="footer"> FOoTR </div>
    </div>
</body>
</html>